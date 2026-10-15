//! The write path: NC/ASM, EQ preset and bands, sidetone and multipoint.
//!
//! These calls change the headset. Every one of them follows the same order: check that
//! the function exists and the value is inside what the capability reports (no frame is
//! sent when it is not), read the current value with a GET, send the SET through a
//! [`Policy`] built for that call alone (the session keeps no opened gate), wait up to
//! the verify wait for the matching notification, read the value back with a GET and
//! compare. A value the headset did not keep is [`WriteError::VerifyFailed`].
//!
//! Run them only with the user's approval.

use std::time::{Duration, Instant};

use mdr::function::{EQ_TYPE_PRESET, NCASM_TYPE_DUAL_AMBIENT};
use mdr::messages::{
    self, ALERT_MULTIPOINT_DISCONNECT, AlertToken, AmbientMode, EQ_PRESET_CUSTOM, EqParam,
    GeneralSettingKind, Message, NcAsmMode, NcAsmParam, Origin, ValueChangeStatus,
};
use mdr::{Policy, PolicyError, Table};
use myhub_rfcomm::ByteLink;
use thiserror::Error;

use crate::session::{READ_POLL_MS, Session, SessionError};

/// How long to wait for the confirmation alert after the multipoint SET.
pub const ALERT_WAIT: Duration = Duration::from_secs(3);
/// How long the link may take to drop after the multipoint confirmation.
pub const DROP_WAIT: Duration = Duration::from_secs(10);

/// Why a write failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WriteError {
    /// The value is outside what the capability reports. Nothing was sent.
    #[error("value out of range: {0}")]
    OutOfRange(&'static str),
    /// The headset does not offer this setting. Nothing was sent.
    #[error("the headset does not support this setting")]
    NotSupported,
    /// The headset did not report the value that was set.
    #[error("the headset did not keep the value")]
    VerifyFailed,
    /// The multipoint SET raised no confirmation alert within [`ALERT_WAIT`].
    #[error("the headset raised no confirmation alert")]
    AlertMissing,
    /// An alert other than the multipoint one came up; it was answered NEGATIVE.
    #[error("the headset raised an unexpected alert")]
    AlertRefused,
    #[error("Bluetooth link disconnected")]
    Disconnected,
    #[error("no reply within the timeout")]
    Timeout,
    #[error("{0}")]
    Session(SessionError),
}

impl From<SessionError> for WriteError {
    fn from(e: SessionError) -> Self {
        match e {
            SessionError::Disconnected | SessionError::LinkDead => Self::Disconnected,
            SessionError::Timeout => Self::Timeout,
            SessionError::Policy(PolicyError::NotSupported { .. }) => Self::NotSupported,
            SessionError::Policy(PolicyError::ParamsNotAllowed { .. }) => {
                Self::OutOfRange("refused by the policy")
            }
            other => Self::Session(other),
        }
    }
}

impl From<PolicyError> for WriteError {
    fn from(e: PolicyError) -> Self {
        SessionError::Policy(e).into()
    }
}

/// The value before the write and the value read back after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change<T> {
    pub before: T,
    pub after: T,
}

/// Result of an EQ band write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqBandsChange {
    /// The EQ state when the call started (it may be another preset than CUSTOM).
    pub before: EqParam,
    /// The bands CUSTOM held right before the new ones were written. Restore with these.
    pub custom_before: Vec<u8>,
    pub after: EqParam,
}

/// What a multipoint change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultipointOutcome {
    /// The headset already had the value; nothing was sent.
    Unchanged,
    /// The change was confirmed and the link dropped. Reconnect and read the value again.
    PendingReconnect { before: bool },
}

fn setting_gate() -> Policy {
    Policy::new().with_setting_changes()
}

fn link_gone(e: &SessionError) -> bool {
    matches!(
        e,
        SessionError::Disconnected | SessionError::LinkDead | SessionError::Transport(_)
    )
}

impl<L: ByteLink> Session<L> {
    fn get_ncasm(&mut self) -> Result<NcAsmParam, WriteError> {
        match self
            .request(&messages::ncasm_get_param(NCASM_TYPE_DUAL_AMBIENT))?
            .message
        {
            Message::NcAsmParam(p) => Ok(p),
            _ => Err(SessionError::Unexpected("NC/ASM param").into()),
        }
    }

    fn get_eq(&mut self) -> Result<EqParam, WriteError> {
        match self.request(&messages::eq_get_param())?.message {
            Message::EqParam(p) => Ok(p),
            _ => Err(SessionError::Unexpected("EQ param").into()),
        }
    }

    fn get_toggle(&mut self, kind: GeneralSettingKind) -> Result<bool, WriteError> {
        match self.request(&messages::general_setting_get(kind))?.message {
            Message::GeneralSetting(g) if g.kind == kind => Ok(g.enabled),
            _ => Err(SessionError::Unexpected("general setting").into()),
        }
    }

    /// Position in the notification queue after the link is idle; only later notifications
    /// count as answers.
    fn mark(&mut self) -> Result<usize, WriteError> {
        self.wait_idle()?;
        Ok(self.notifications_mut().len())
    }

    /// Waits up to the verify wait for a notification after `mark` that `matches`.
    /// `false` is not an error: the caller reads the value back either way.
    fn wait_ntfy(
        &mut self,
        mark: usize,
        matches: impl Fn(&Message) -> bool,
    ) -> Result<bool, WriteError> {
        let deadline = Instant::now() + self.verify_wait();
        loop {
            if self
                .notifications_mut()
                .iter()
                .skip(mark)
                .any(|n| n.origin == Origin::Notification && matches(&n.message))
            {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            self.poll(READ_POLL_MS)?;
        }
    }

    /// Takes the first alert that arrived after `mark` out of the queue.
    fn take_alert(&mut self, mark: usize) -> Option<(u8, Option<AlertToken>)> {
        let queue = self.notifications_mut();
        let index = (mark..queue.len()).find(|&i| matches!(queue[i].message, Message::Alert(_)))?;
        let mut incoming = queue.remove(index)?;
        match &mut incoming.message {
            Message::Alert(a) => Some((a.message_id, a.take_token())),
            _ => None,
        }
    }

    fn eq_ready(&self) -> Result<(), WriteError> {
        if self.functions().has_eq_type(EQ_TYPE_PRESET) && self.eq_capability().is_some() {
            Ok(())
        } else {
            Err(WriteError::NotSupported)
        }
    }

    /// Sets noise cancelling or ambient sound. Status CHANGED, total effect on. In noise
    /// cancelling mode the level sent is 0 (as the app does; the device stores a level per
    /// mode) and `level` is not used. In ambient mode `level` must be inside the range
    /// and on the step of `ambient_mode`.
    pub fn set_nc_asm(
        &mut self,
        mode: NcAsmMode,
        ambient_mode: AmbientMode,
        level: u8,
    ) -> Result<Change<NcAsmParam>, WriteError> {
        if !self.functions().has_ncasm_type(NCASM_TYPE_DUAL_AMBIENT) {
            return Err(WriteError::NotSupported);
        }
        let capability = self.ncasm_capability().ok_or(WriteError::NotSupported)?;
        let range = capability
            .ranges()
            .iter()
            .find(|r| r.ambient_mode() == ambient_mode)
            .ok_or(WriteError::OutOfRange("ambient mode"))?;
        let sent_level = match mode {
            NcAsmMode::NoiseCancelling => 0,
            NcAsmMode::Ambient => {
                let on_step = range.step() == 0
                    || (level >= range.min() && (level - range.min()).is_multiple_of(range.step()));
                if !(range.min()..=range.max()).contains(&level) || !on_step {
                    return Err(WriteError::OutOfRange("ambient level"));
                }
                level
            }
        };
        let before = self.get_ncasm()?;
        let param = NcAsmParam {
            status: ValueChangeStatus::Changed,
            total_effect: true,
            mode,
            ambient_mode,
            level: sent_level,
        };
        let mark = self.mark()?;
        self.send_with(
            setting_gate(),
            &messages::ncasm_set_param(NCASM_TYPE_DUAL_AMBIENT, &param),
        )?;
        self.wait_ntfy(mark, |m| matches!(m, Message::NcAsmParam(_)))?;
        let after = self.get_ncasm()?;
        let kept = after.mode == mode
            && (mode == NcAsmMode::NoiseCancelling
                || (after.ambient_mode == ambient_mode && after.level == level));
        if kept {
            Ok(Change { before, after })
        } else {
            Err(WriteError::VerifyFailed)
        }
    }

    /// Selects the EQ preset with `58 00 <id> 00`; the id must be in the capability.
    pub fn select_eq_preset(&mut self, id: u8) -> Result<Change<EqParam>, WriteError> {
        self.eq_ready()?;
        if !self
            .eq_capability()
            .is_some_and(|c| c.presets().iter().any(|p| p.id() == id))
        {
            return Err(WriteError::OutOfRange("preset id"));
        }
        let before = self.get_eq()?;
        let (after, _) = self.select_preset(id)?;
        Ok(Change { before, after })
    }

    /// Sends the select frame, waits for the notification, reads back and checks.
    fn select_preset(&mut self, id: u8) -> Result<(EqParam, usize), WriteError> {
        let mark = self.mark()?;
        self.send_with(setting_gate(), &messages::eq_select_preset(id))?;
        self.wait_ntfy(mark, |m| {
            matches!(m, Message::EqParam(_) | Message::EqChanged)
        })?;
        let after = self.get_eq()?;
        // A later notification that names another preset contradicts the read-back.
        let contradicted = self
            .notifications_mut()
            .iter()
            .skip(mark)
            .any(|n| matches!(&n.message, Message::EqParam(p) if p.preset != id));
        if after.preset == id && !contradicted {
            Ok((after, self.notifications_mut().len()))
        } else {
            Err(WriteError::VerifyFailed)
        }
    }

    /// Writes the bands of CUSTOM, one step per band (`0..step count`). When another
    /// preset is active CUSTOM is selected first and checked: the headset ignores or
    /// misfiles bands sent while another preset is active, so they are never sent then.
    pub fn set_eq_bands(&mut self, steps: &[u8]) -> Result<EqBandsChange, WriteError> {
        self.eq_ready()?;
        let capability = self.eq_capability().ok_or(WriteError::NotSupported)?;
        if !capability
            .presets()
            .iter()
            .any(|p| p.id() == EQ_PRESET_CUSTOM)
        {
            return Err(WriteError::NotSupported);
        }
        if steps.len() != usize::from(capability.band_count()) {
            return Err(WriteError::OutOfRange("band count"));
        }
        if steps.iter().any(|&s| s >= capability.step_count()) {
            return Err(WriteError::OutOfRange("band step"));
        }
        let before = self.get_eq()?;
        let (custom_before, end) = if before.preset == EQ_PRESET_CUSTOM {
            (before.bands.clone(), self.notifications_mut().len())
        } else {
            let (custom, end) = self.select_preset(EQ_PRESET_CUSTOM)?;
            (custom.bands, end)
        };
        // Take in what is still pending, then make sure nothing since the CUSTOM select
        // says another preset is active: bands sent then would be ignored or misfiled.
        self.wait_idle()?;
        self.poll(READ_POLL_MS)?;
        let moved = self
            .notifications_mut()
            .iter()
            .skip(end)
            .any(|n| matches!(&n.message, Message::EqParam(p) if p.preset != EQ_PRESET_CUSTOM));
        if moved {
            return Err(WriteError::VerifyFailed);
        }
        let mark = self.mark()?;
        self.send_with(setting_gate(), &messages::eq_set_bands(steps))?;
        self.wait_ntfy(mark, |m| {
            matches!(m, Message::EqParam(_) | Message::EqChanged)
        })?;
        let after = self.get_eq()?;
        if after.preset == EQ_PRESET_CUSTOM && after.bands == steps {
            Ok(EqBandsChange {
                before,
                custom_before,
                after,
            })
        } else {
            Err(WriteError::VerifyFailed)
        }
    }

    /// Sidetone on or off (`d8 d2 00 <00 on | 01 off>`).
    pub fn set_sidetone(&mut self, on: bool) -> Result<Change<bool>, WriteError> {
        let kind = GeneralSettingKind::Sidetone;
        if !self.functions().contains(Table::One, kind.to_u8()) {
            return Err(WriteError::NotSupported);
        }
        let before = self.get_toggle(kind)?;
        let mark = self.mark()?;
        self.send_with(setting_gate(), &messages::general_setting_set(kind, on))?;
        self.wait_ntfy(
            mark,
            |m| matches!(m, Message::GeneralSetting(g) if g.kind == kind),
        )?;
        let after = self.get_toggle(kind)?;
        if after == on {
            Ok(Change { before, after })
        } else {
            Err(WriteError::VerifyFailed)
        }
    }

    /// Multipoint on or off. The headset raises alert 0x07 and drops the link a few seconds
    /// after the confirmation. The SET goes through the disconnecting gate; the alert is
    /// answered POSITIVE (same gate) only for message 0x07, any other alert is answered
    /// NEGATIVE (setting gate) and gives [`WriteError::AlertRefused`]. Without an alert
    /// within [`ALERT_WAIT`] nothing more is sent ([`WriteError::AlertMissing`]). After
    /// [`MultipointOutcome::PendingReconnect`] the caller opens a new link and reads again.
    pub fn set_multipoint(&mut self, on: bool) -> Result<MultipointOutcome, WriteError> {
        let kind = GeneralSettingKind::Multipoint;
        if !self.functions().contains(Table::One, kind.to_u8()) {
            return Err(WriteError::NotSupported);
        }
        let before = self.get_toggle(kind)?;
        if before == on {
            return Ok(MultipointOutcome::Unchanged);
        }
        let mark = self.mark()?;
        self.send_with(
            Policy::new().with_disconnecting_changes(),
            &messages::general_setting_set(kind, on),
        )?;
        let alert_deadline = Instant::now() + ALERT_WAIT;
        let (id, token) = loop {
            if let Some(alert) = self.take_alert(mark) {
                break alert;
            }
            if Instant::now() >= alert_deadline {
                return Err(WriteError::AlertMissing);
            }
            self.poll(READ_POLL_MS)?;
        };
        let token = match token {
            Some(token) if id == ALERT_MULTIPOINT_DISCONNECT => token,
            other => {
                // Not the expected alert: refuse it when it can be answered at all.
                if let Some(token) = other {
                    let negative = setting_gate().check_alert_answer(token, false)?;
                    self.send_checked(negative)?;
                }
                return Err(WriteError::AlertRefused);
            }
        };
        let positive = Policy::new()
            .with_disconnecting_changes()
            .check_alert_answer(token, true)?;
        self.send_checked(positive)?;
        let deadline = Instant::now() + DROP_WAIT;
        while Instant::now() < deadline {
            match self.poll(READ_POLL_MS) {
                Ok(()) => {}
                Err(e) if link_gone(&e) => {
                    return Ok(MultipointOutcome::PendingReconnect { before });
                }
                Err(e) => return Err(e.into()),
            }
        }
        // Confirmed but the link stayed up: the state is unknown, the caller reads again.
        Err(WriteError::Timeout)
    }
}
