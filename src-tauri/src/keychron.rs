//! Keyboard worker: one background thread ("keychron-worker") shows the Keychron K2 HE
//! next to the other devices. Writes (USB only, `allowDeviceWrites` on): HE profile select,
//! global travel of the current profile, global lighting; see [`KeyboardWrite`].
//!
//! Discovery does not talk to the keyboard: the worker enumerates HID interfaces every
//! 2 s while the keyboard is not connected. USB has priority. When the keyboard's raw HID
//! interface shows up it opens it and reads the whole snapshot once (and caches it, see
//! below). Until that read succeeds the device is absent from the list (same as the
//! headset).
//!
//! While connected over USB the worker sends nothing by itself: the Keychron launcher may
//! use the same interface and frames carry no sequence number, so a reply cannot be told
//! from the launcher's reply. It only listens (`Session::poll`) and re-enumerates every
//! 2 s to notice an unplug. A `refresh_device` re-reads the current profile and the
//! lighting values (Read tier); profile changes made on the keyboard are not pushed, so
//! the UI asks for a refresh when the panel opens.
//!
//! Without the cable, when the Keychron Link receiver's raw HID interface is listed, the
//! worker opens a receiver session, reads B1..B3 once and then B2 (the link state) every
//! [`LINK_POLL`]. A linked K2 HE is shown as connected through the receiver with the
//! settings of the last USB read (the cache file `cache/keychron-k2he.json` in the app
//! config dir, written after every USB read); a keyboard frame is never sent over the
//! receiver. Plugging the cable in closes the receiver session and switches to USB.
//!
//! An unrequested report that is not a known event, or a failed request because of one,
//! means another program talks to the keyboard (or the receiver): the detail gets
//! `otherConfigurator`, the worker sends nothing and goes on listening, and reads again
//! only after [`FOREIGN_QUIET`] without a foreign report. No serial number and no device
//! path is logged: this module never prints one.

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use keychron_k2he::{
    DEVICE_ID, Incoming, KeyWord, KeyboardCache, LightChange, LightState, LinkSnapshot, Presence,
    RawLink, SaveMode, Session, SessionError, Snapshot, TravelMode, TravelSettings, cache_of,
    cache_path, presence, read_global_word, read_light_state, read_link, read_link_state,
    read_profiles_info, read_snapshot, refresh_state, select_profile, set_lighting_with,
    set_travel_global_with, to_detail, to_receiver_detail,
};

use myhub_core::{Connection, DeviceDetail};
use serde::Deserialize;
use tauri::Manager;

use crate::device_manager::{FlashFlag, ManagerState, Slot, clock_ms, lock, publish_slot};
use crate::hotplug;
use crate::settings::SettingsState;

// A full HID enumeration is expensive on Windows (SetupDi plus a CreateFile on every HID
// device, audio devices' HID interfaces included). Run every 2 s it is the likely cause
// [INF] of the system audio glitches seen while MyHUB was open [OBS]. So: no enumeration
// while a USB session is open (an unplug shows up as a read error), and only the
// intervals below otherwise.

/// Pause between enumerations while neither the keyboard nor the receiver is found (and
/// after a receiver that could not be used).
const IDLE_SCAN_INTERVAL: Duration = Duration::from_secs(10);
/// Pause after a session that was lost, once, before the next look.
const RECONNECT_PAUSE: Duration = Duration::from_secs(2);
/// A USB session at least this long counts as a success for the back off.
const HEALTHY_SESSION: Duration = Duration::from_secs(30);

/// Pause after the n-th consecutive connect failure or quick loss: 2 s, 5 s, then the idle
/// scan interval (a slow fallback when hotplug is active).
fn connect_backoff(failures: u32) -> Duration {
    match failures {
        0..=1 => RECONNECT_PAUSE,
        2 => Duration::from_secs(5),
        _ => hotplug::scan_interval(IDLE_SCAN_INTERVAL),
    }
}

/// Gap between the checks for the cable (and the receiver) while a receiver session is
/// open. A Refresh command from the UI checks at once.
const CABLE_CHECK_INTERVAL: Duration = Duration::from_secs(15);
/// Pause after a connect that failed because another program was talking.
const FOREIGN_CONNECT_WAIT: Duration = Duration::from_secs(10);
/// One listen of the serving loop; also the latency of a refresh command and of
/// shutdown.
const POLL: Duration = Duration::from_millis(200);
/// No foreign report for this long before the worker sends anything again.
pub const FOREIGN_QUIET: Duration = Duration::from_secs(10);
/// Gap between two B2 reads over the receiver. The receiver sends no link-change report
/// [OBS, `read_link_state`], so B2 is the only way to see the keyboard come and go; a
/// handful of seconds of latency in a status line is not worth a request every 5 s.
pub const LINK_POLL: Duration = Duration::from_secs(15);

/// What the keyboard worker accepts.
pub enum KeychronCommand {
    /// Re-read the profile and lighting values and answer with the new detail (the
    /// cached one while another program talks to the keyboard).
    Refresh { reply: Sender<Option<DeviceDetail>> },
    /// A write to the keyboard (profile select, travel, lighting). Only over USB. The
    /// answer is the new detail or a contract error code.
    Write {
        op: KeyboardWrite,
        /// The caller's give-up instant minus a margin; a write starts only with the
        /// budget of its kind left ([`MIN_SELECT_BUDGET`], [`MIN_SETTINGS_BUDGET`]).
        deadline: Instant,
        reply: Sender<Result<DeviceDetail, String>>,
    },
    /// The settings changed: publish the detail again (`readOnly` may flip).
    SettingsChanged,
    /// Wakes the worker (shutdown).
    Wake,
}

/// How long the caller waits for a profile selection. The worker deadline is this minus the
/// 2 s margin.
pub const KEYBOARD_WRITE_WAIT: Duration = Duration::from_secs(15);

/// How long the caller waits for a travel or lighting write (deadline: this minus 2 s).
pub const KEYBOARD_SETTINGS_WAIT: Duration = Duration::from_secs(25);

/// A profile selection starts only when at least this much of the deadline is left. The
/// reads inside `select_profile` are single attempts now, so its worst case is about 6 s
/// (A9 10, A9 11, A9 10 at 1 s each, then a re-read A9 10 with its resends up to 3 s); the
/// old 11 s is kept, it could be lowered to about 7 s.
const MIN_SELECT_BUDGET: Duration = Duration::from_secs(11);

/// A travel or lighting write starts only when at least this much of the deadline is left.
/// With single-attempt reads every frame costs at most one 1 s reply timeout: travel is at
/// most 7 frames, lighting at most 17 (12 reads, 4 VIA 07, VIA 09 03), plus two 250 ms
/// write gaps per extra write frame and the re-read after an error.
const MIN_SETTINGS_BUDGET: Duration = Duration::from_secs(20);

/// Newest effect number of the keyboard's global lighting [OBS effectMax 24].
const EFFECT_MAX: i64 = 24;

/// `mode` argument of `set_keyboard_travel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TravelModeArg {
    Static,
    RapidTrigger,
}

/// A write to the keyboard, already range checked (the profile index is checked against
/// the profile count in the worker).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardWrite {
    /// 0-based HE profile index, as sent by the UI.
    Profile(i64),
    /// The settings and the 0-based profile the UI edits (the keyboard must still be on it).
    Travel {
        settings: TravelSettings,
        profile: u8,
    },
    Lighting(LightChange),
}

impl KeyboardWrite {
    /// How long the caller of this write waits.
    pub fn wait(&self) -> Duration {
        match self {
            Self::Profile(_) => KEYBOARD_WRITE_WAIT,
            Self::Travel { .. } | Self::Lighting(_) => KEYBOARD_SETTINGS_WAIT,
        }
    }

    fn min_budget(&self) -> Duration {
        match self {
            Self::Profile(_) => MIN_SELECT_BUDGET,
            Self::Travel { .. } | Self::Lighting(_) => MIN_SETTINGS_BUDGET,
        }
    }

    /// A travel write from millimetres: 0.1 mm units (rounded), actuation 0.2..=3.8 mm,
    /// press and release 0.1..=3.8 mm (Rapid Trigger only, ignored and unchecked in Static
    /// mode). `profile` (0-based, the profile the UI shows as active) must fit a byte. `outOfRange`
    /// otherwise.
    pub fn travel(
        profile: i64,
        mode: TravelModeArg,
        actuation_mm: f64,
        press_mm: f64,
        release_mm: f64,
    ) -> Result<Self, String> {
        let tenths = |mm: f64, min: u8| -> Result<u8, String> {
            let t = (mm * 10.0).round();
            if mm.is_finite() && t >= f64::from(min) && t <= 38.0 {
                Ok(t as u8)
            } else {
                Err("outOfRange".to_owned())
            }
        };
        let profile = u8::try_from(profile).map_err(|_| "outOfRange".to_owned())?;
        let actuation = tenths(actuation_mm, 2)?;
        let (mode, press, release) = match mode {
            TravelModeArg::Static => (TravelMode::Static, 0, 0),
            TravelModeArg::RapidTrigger => (
                TravelMode::RapidTrigger,
                tenths(press_mm, 1)?,
                tenths(release_mm, 1)?,
            ),
        };
        Ok(Self::Travel {
            settings: TravelSettings {
                mode,
                actuation,
                press,
                release,
            },
            profile,
        })
    }

    /// A lighting write: effect 0..=24, brightness, speed, hue and saturation 0..=255;
    /// hue and saturation together or neither; at least one field. `outOfRange` otherwise.
    pub fn lighting(
        effect: Option<i64>,
        brightness: Option<i64>,
        speed: Option<i64>,
        hue: Option<i64>,
        saturation: Option<i64>,
    ) -> Result<Self, String> {
        let bad = || "outOfRange".to_owned();
        let byte = |v: i64| u8::try_from(v).map_err(|_| bad());
        let effect = effect
            .map(|v| {
                if (0..=EFFECT_MAX).contains(&v) {
                    byte(v)
                } else {
                    Err(bad())
                }
            })
            .transpose()?;
        let brightness = brightness.map(byte).transpose()?;
        let speed = speed.map(byte).transpose()?;
        let color = match (hue, saturation) {
            (Some(h), Some(s)) => Some((byte(h)?, byte(s)?)),
            (None, None) => None,
            _ => return Err(bad()),
        };
        let change = LightChange {
            effect,
            brightness,
            speed,
            color,
        };
        if change == LightChange::default() {
            return Err(bad());
        }
        Ok(Self::Lighting(change))
    }
}

/// A queued keyboard write.
struct PendingWrite {
    op: KeyboardWrite,
    deadline: Instant,
    reply: Sender<Result<DeviceDetail, String>>,
}

/// What a keyboard write did to the keyboard state.
struct WriteResult {
    /// `Ok` or the contract error code.
    answer: Result<(), String>,
    /// The cached snapshot or the foreign flag changed.
    changed: bool,
    /// The link is gone (reason).
    lost: Option<String>,
}

/// What a failed travel or lighting write re-reads.
#[derive(Clone, Copy)]
enum Reread {
    Travel,
    /// The global word of this profile only.
    Word(u8),
    Lighting,
}

/// `deviceError:budget` when less of the deadline is left than `op` needs; nothing is sent.
fn budget_refusal(op: KeyboardWrite, deadline: Instant, now: Instant) -> Option<WriteResult> {
    (deadline.saturating_duration_since(now) < op.min_budget())
        .then(|| WriteResult::refused("deviceError:budget"))
}

impl WriteResult {
    fn ok() -> Self {
        Self {
            answer: Ok(()),
            changed: false,
            lost: None,
        }
    }

    fn refused(code: &str) -> Self {
        Self {
            answer: Err(code.to_owned()),
            changed: false,
            lost: None,
        }
    }
}

/// The error code of a write that cannot reach a USB session: writes off first, then a
/// receiver that links the keyboard (`usbRequired`), otherwise `disconnected`.
fn no_usb_refusal(writes_allowed: bool, receiver_linked: bool) -> &'static str {
    if !writes_allowed {
        "writesDisabled"
    } else if receiver_linked {
        "usbRequired"
    } else {
        "disconnected"
    }
}

/// The `<kind>` of `deviceError:<kind>` for a session error.
fn error_kind(error: &SessionError) -> &'static str {
    match error {
        SessionError::Timeout => "timeout",
        SessionError::AmbiguousReply => "ambiguous",
        SessionError::BadReport(_) => "badReport",
        SessionError::Link(_) => "link",
        SessionError::Policy(_) => "policy",
        SessionError::Decode(_) => "decode",
        SessionError::DeviceStatus { .. } => "status",
        _ => "session",
    }
}

/// An open session with its cached snapshot and the foreign traffic state. Generic over
/// the link so tests can use a mock.
struct Keyboard<L: RawLink> {
    session: Session<L>,
    snapshot: Snapshot,
    /// When the last foreign report or foreign failure was seen; `Some` means another
    /// configurator is active and nothing is sent.
    last_foreign: Option<Instant>,
}

impl<L: RawLink> Keyboard<L> {
    fn new(session: Session<L>, snapshot: Snapshot) -> Self {
        Self {
            session,
            snapshot,
            last_foreign: None,
        }
    }

    fn other_configurator(&self) -> bool {
        self.last_foreign.is_some()
    }

    fn detail(&self, connected: bool) -> DeviceDetail {
        to_detail(
            &self.snapshot,
            connected,
            connected && self.other_configurator(),
        )
    }

    /// Foreign traffic seen at `now`. `true` when this starts the foreign state.
    fn mark_foreign(&mut self, now: Instant) -> bool {
        self.last_foreign.replace(now).is_none()
    }

    /// A failed request or listen. Foreign traffic starts or extends the quiet period
    /// (`Ok(true)` when the flag flipped), a link error ends the session (`Err`), any
    /// other failure is logged.
    fn failed(&mut self, error: &SessionError, now: Instant) -> Result<bool, String> {
        match error {
            SessionError::ForeignTraffic
            | SessionError::BackingOff { .. }
            | SessionError::BadReport(_) => Ok(self.mark_foreign(now)),
            SessionError::Link(_) => Err(error.to_string()),
            other => {
                eprintln!("myhub: keyboard read failed: {other}");
                // A failed re-read after the quiet period: wait another one.
                if self.last_foreign.is_some() {
                    self.last_foreign = Some(now);
                }
                Ok(false)
            }
        }
    }

    /// Select the HE profile `profile` (0-based). Preconditions, before any frame:
    /// writes allowed, no other configurator, `profile` below the profile count, enough
    /// budget. Runs `select_profile(.., save = false)`: A9 11 only, never A9 1F, no retry.
    /// A failure after a frame may have gone out re-reads A9 10 here, so the snapshot is
    /// known again before anything else is served.
    fn select_profile(
        &mut self,
        writes_allowed: bool,
        profile: i64,
        deadline: Instant,
        now: Instant,
    ) -> WriteResult {
        if let Some(refusal) = self.refuse_early(writes_allowed) {
            return refusal;
        }
        let count = i64::from(self.snapshot.profiles_info.count);
        let Some(index) = u8::try_from(profile).ok().filter(|p| i64::from(*p) < count) else {
            return WriteResult::refused("outOfRange");
        };
        if let Some(refusal) = budget_refusal(KeyboardWrite::Profile(profile), deadline, now) {
            return refusal;
        }
        let mut result = WriteResult::ok();
        match select_profile(&mut self.session, index, false) {
            Ok(outcome) => {
                result.changed = self.patch_profile(outcome.after);
            }
            Err(SessionError::VerifyFailed { got, .. }) => {
                // The keyboard told us what it is on now.
                result.changed = self.patch_profile(got);
                result.answer = Err("verifyFailed".to_owned());
            }
            Err(
                SessionError::ForeignTraffic
                | SessionError::BackingOff { .. }
                | SessionError::BadReport(_),
            ) => {
                result.changed = self.mark_foreign(now);
                result.answer = Err("busy".to_owned());
            }
            Err(error @ SessionError::Link(_)) => {
                result.answer = Err(format!("deviceError:{}", error_kind(&error)));
                result.lost = Some(error.to_string());
            }
            Err(error) => {
                result.answer = Err(format!("deviceError:{}", error_kind(&error)));
                // No frame went out for a policy refusal; anything else may have
                // changed the profile, so read it again.
                if !matches!(error, SessionError::Policy(_)) {
                    match read_profiles_info(&mut self.session) {
                        Ok(info) => result.changed = self.patch_profile(info.current),
                        Err(e) => match self.failed(&e, now) {
                            Ok(flipped) => result.changed = flipped,
                            Err(reason) => result.lost = Some(reason),
                        },
                    }
                }
            }
        }
        result
    }

    /// The first two preconditions of every write, in order: writes allowed, no other
    /// configurator.
    fn refuse_early(&self, writes_allowed: bool) -> Option<WriteResult> {
        if !writes_allowed {
            Some(WriteResult::refused("writesDisabled"))
        } else if self.other_configurator() {
            Some(WriteResult::refused("busy"))
        } else {
            None
        }
    }

    /// Set the global travel of the CURRENT profile and save it (A9 14, A9 1F). The values
    /// are range checked by the caller. Preconditions: writes allowed, no other
    /// configurator, enough budget. Success patches the current profile's global word from
    /// the read-back.
    fn set_travel(
        &mut self,
        writes_allowed: bool,
        settings: TravelSettings,
        profile: u8,
        deadline: Instant,
        now: Instant,
    ) -> WriteResult {
        if let Some(refusal) = self.refuse_early(writes_allowed) {
            return refusal;
        }
        if let Some(refusal) =
            budget_refusal(KeyboardWrite::Travel { settings, profile }, deadline, now)
        {
            return refusal;
        }
        let mut result = WriteResult::ok();
        // Save only when a write was needed; the keyboard must still be on the profile the UI edits.
        match set_travel_global_with(
            &mut self.session,
            settings,
            SaveMode::IfChanged,
            Some(profile),
        ) {
            Ok(outcome) => {
                result.changed = self.patch_profile(outcome.profile);
                result.changed |= self.patch_travel(outcome.profile, outcome.after);
            }
            Err(error) => self.write_failed(error, now, Reread::Travel, &mut result),
        }
        result
    }

    /// Change the global lighting and save it (VIA 07 03 per field, VIA 09 03). Same
    /// preconditions as [`Keyboard::set_travel`].
    fn set_lighting(
        &mut self,
        writes_allowed: bool,
        change: LightChange,
        deadline: Instant,
        now: Instant,
    ) -> WriteResult {
        if let Some(refusal) = self.refuse_early(writes_allowed) {
            return refusal;
        }
        if let Some(refusal) = budget_refusal(KeyboardWrite::Lighting(change), deadline, now) {
            return refusal;
        }
        let mut result = WriteResult::ok();
        match set_lighting_with(&mut self.session, change, SaveMode::IfChanged) {
            Ok(outcome) => result.changed = self.patch_light(outcome.after),
            Err(error) => self.write_failed(error, now, Reread::Lighting, &mut result),
        }
        result
    }

    /// Runs one queued write.
    fn write(
        &mut self,
        op: KeyboardWrite,
        writes_allowed: bool,
        deadline: Instant,
        now: Instant,
    ) -> WriteResult {
        match op {
            KeyboardWrite::Profile(profile) => {
                self.select_profile(writes_allowed, profile, deadline, now)
            }
            KeyboardWrite::Travel { settings, profile } => {
                self.set_travel(writes_allowed, settings, profile, deadline, now)
            }
            KeyboardWrite::Lighting(change) => {
                self.set_lighting(writes_allowed, change, deadline, now)
            }
        }
    }

    /// The error of a travel or lighting write. A mismatch, a status error or a timeout
    /// leaves the state unknown, so what the write touched is read again (the first failed
    /// read ends the re-read). A policy refusal and a device status sent no change.
    fn write_failed(
        &mut self,
        error: SessionError,
        now: Instant,
        reread: Reread,
        result: &mut WriteResult,
    ) {
        match error {
            SessionError::VerifyMismatch { saved, .. } => {
                let code = if saved {
                    "verifyFailed:saved"
                } else {
                    "verifyFailed"
                };
                result.answer = Err(code.to_owned());
                self.reread(reread, now, result);
            }
            SessionError::ProfileChanged { expected, got } => {
                result.changed = self.patch_profile(got);
                result.answer = Err("profileChanged".to_owned());
                // If A9 14 went out before the profile changed, the old profile's word in RAM
                // may differ from the cache: read it again (one frame).
                self.reread(Reread::Word(expected), now, result);
            }
            SessionError::ForeignTraffic
            | SessionError::BackingOff { .. }
            | SessionError::BadReport(_) => {
                result.changed = self.mark_foreign(now);
                result.answer = Err("busy".to_owned());
            }
            SessionError::Link(_) => {
                result.answer = Err(format!("deviceError:{}", error_kind(&error)));
                result.lost = Some(error.to_string());
            }
            // A9 1F refused after A9 14 went out: RAM holds the new value, read it again.
            SessionError::DeviceStatus { sub: 0x1F, .. } => {
                result.answer = Err(format!("deviceError:{}", error_kind(&error)));
                self.reread(reread, now, result);
            }
            // A refused A9 14 or a policy refusal changed nothing.
            SessionError::DeviceStatus { .. } | SessionError::Policy(_) => {
                result.answer = Err(format!("deviceError:{}", error_kind(&error)));
            }
            other => {
                result.answer = Err(format!("deviceError:{}", error_kind(&other)));
                self.reread(reread, now, result);
            }
        }
    }

    /// Reads again what a failed write touched and patches the snapshot.
    fn reread(&mut self, reread: Reread, now: Instant, result: &mut WriteResult) {
        let read = match reread {
            Reread::Travel => read_profiles_info(&mut self.session).and_then(|info| {
                let word = read_global_word(&mut self.session, info.current)?;
                let mut changed = self.patch_profile(info.current);
                changed |= self.patch_travel(info.current, word);
                Ok(changed)
            }),
            Reread::Word(profile) => read_global_word(&mut self.session, profile)
                .map(|word| self.patch_travel(profile, word)),
            Reread::Lighting => {
                read_light_state(&mut self.session).map(|state| self.patch_light(state))
            }
        };
        match read {
            Ok(changed) => result.changed |= changed,
            Err(e) => match self.failed(&e, now) {
                Ok(flipped) => result.changed |= flipped,
                Err(reason) => result.lost = Some(reason),
            },
        }
    }

    /// Sets the global word of `profile`; `true` when it changed.
    fn patch_travel(&mut self, profile: u8, word: KeyWord) -> bool {
        match self.snapshot.profiles.get_mut(usize::from(profile)) {
            Some(p) if p.global != word => {
                p.global = word;
                true
            }
            _ => false,
        }
    }

    /// Sets the cached lighting values; `true` when one changed.
    fn patch_light(&mut self, state: LightState) -> bool {
        let fresh = keychron_k2he::LightSettings {
            brightness: Some(state.brightness),
            effect: Some(state.effect),
            speed: Some(state.speed),
            hue: Some(state.hue),
            saturation: Some(state.saturation),
        };
        let changed = self.snapshot.light != fresh;
        self.snapshot.light = fresh;
        changed
    }

    /// Sets the cached current profile; `true` when it changed.
    fn patch_profile(&mut self, current: u8) -> bool {
        let changed = self.snapshot.profiles_info.current != current;
        self.snapshot.profiles_info.current = current;
        changed
    }

    /// One pass of the serving loop: the light refresh if asked (and nobody else is
    /// talking), one listen of up to `listen`, the events folded into the snapshot, and
    /// the full re-read once the keyboard was quiet for [`FOREIGN_QUIET`]. `Ok(true)`
    /// when the detail changed, `Err` with the reason when the link is gone.
    fn step(&mut self, now: Instant, refresh: bool, listen: Duration) -> Result<bool, String> {
        let mut changed = false;
        if refresh && !self.other_configurator() {
            match refresh_state(&mut self.session, &mut self.snapshot) {
                Ok(()) => changed = true,
                Err(e) => changed |= self.failed(&e, now)?,
            }
        }
        match self.session.poll(listen) {
            Ok(Some(Incoming::Foreign(_))) => changed |= self.mark_foreign(now),
            Ok(_) => {}
            Err(e) => changed |= self.failed(&e, now)?,
        }
        for event in self.session.drain_events() {
            changed |= self.snapshot.apply_event(event);
        }
        if self
            .last_foreign
            .is_some_and(|last| now.saturating_duration_since(last) >= FOREIGN_QUIET)
        {
            match read_snapshot(&mut self.session) {
                Ok(fresh) => {
                    self.snapshot = fresh;
                    self.last_foreign = None;
                    changed = true;
                }
                Err(e) => changed |= self.failed(&e, now)?,
            }
        }
        Ok(changed)
    }
}

/// An open receiver session: the link state of the keyboard behind the Keychron Link
/// receiver. Only B2 is ever sent after the first read; generic over the link for tests.
struct Link<L: RawLink> {
    session: Session<L>,
    /// B3 of the first read.
    firmware: Option<String>,
    /// B2 says the linked device is a connected K2 HE.
    linked: bool,
    /// When foreign traffic was last seen; `Some` means nothing is sent.
    last_foreign: Option<Instant>,
    /// When B2 is read next.
    next_read: Instant,
    /// The last B2 read failed for a reason that is not foreign traffic (logged once).
    read_failing: bool,
}

impl<L: RawLink> Link<L> {
    fn new(session: Session<L>, first: &LinkSnapshot, now: Instant) -> Self {
        Self {
            session,
            firmware: first.firmware.clone(),
            linked: first.state.is_k2he(),
            last_foreign: None,
            next_read: now + LINK_POLL,
            read_failing: false,
        }
    }

    fn other_configurator(&self) -> bool {
        self.last_foreign.is_some()
    }

    fn detail(&self, cache: Option<&KeyboardCache>, connected: bool) -> DeviceDetail {
        to_receiver_detail(
            cache,
            self.firmware.clone(),
            connected,
            self.other_configurator(),
        )
    }

    fn mark_foreign(&mut self, now: Instant) -> bool {
        self.last_foreign.replace(now).is_none()
    }

    /// A failed read or listen, as for the USB session.
    fn failed(&mut self, error: &SessionError, now: Instant) -> Result<bool, String> {
        match error {
            SessionError::ForeignTraffic
            | SessionError::BackingOff { .. }
            | SessionError::BadReport(_) => Ok(self.mark_foreign(now)),
            SessionError::Link(_) => Err(error.to_string()),
            other => {
                if !self.read_failing {
                    eprintln!("myhub: keyboard link read failed: {other}");
                    self.read_failing = true;
                }
                if self.last_foreign.is_some() {
                    self.last_foreign = Some(now);
                }
                Ok(false)
            }
        }
    }

    /// One pass of the serving loop: B2 when due or asked for (and nobody else is
    /// talking), then one listen of up to `listen`. `Ok(true)` when the detail changed,
    /// `Err` with the reason when the link is gone. Nothing but B2 is ever written.
    fn step(&mut self, now: Instant, refresh: bool, listen: Duration) -> Result<bool, String> {
        let mut changed = false;
        // While another program talks nothing is sent; once it was quiet for
        // FOREIGN_QUIET one B2 read is tried and the flag clears when it succeeds.
        let quiet = self
            .last_foreign
            .is_some_and(|last| now.saturating_duration_since(last) >= FOREIGN_QUIET);
        if (refresh || now >= self.next_read) && (quiet || !self.other_configurator()) {
            self.next_read = now + LINK_POLL;
            match read_link_state(&mut self.session) {
                Ok(state) => {
                    self.read_failing = false;
                    let linked = state.is_k2he();
                    changed |= linked != self.linked || self.last_foreign.take().is_some();
                    self.linked = linked;
                }
                Err(e) => changed |= self.failed(&e, now)?,
            }
        }
        match self.session.poll(listen) {
            Ok(Some(Incoming::Foreign(_))) => changed |= self.mark_foreign(now),
            Ok(_) => {}
            Err(e) => changed |= self.failed(&e, now)?,
        }
        // The receiver sends no keyboard events.
        drop(self.session.drain_events());
        Ok(changed)
    }
}

/// What is listed right now, from one enumeration. An enumeration failure counts as
/// nothing found.
fn scan() -> Presence {
    presence().unwrap_or(Presence {
        keyboard: false,
        receiver: false,
    })
}

/// [`scan`] behind the module switch: `None` without calling `enumerate` when it is off.
fn scan_if_enabled(enabled: bool, enumerate: impl FnOnce() -> Presence) -> Option<Presence> {
    enabled.then(enumerate)
}

/// When the receiver loop checks for the cable next.
struct CableCheck {
    next: Instant,
}

impl CableCheck {
    fn new(now: Instant) -> Self {
        Self {
            next: now + hotplug::scan_interval(CABLE_CHECK_INTERVAL),
        }
    }

    /// True when a check is due (or `forced`, a user-driven refresh); re-arms itself.
    fn due(&mut self, now: Instant, forced: bool) -> bool {
        if forced || now >= self.next {
            self.next = now + hotplug::scan_interval(CABLE_CHECK_INTERVAL);
            true
        } else {
            false
        }
    }
}

/// Why no session could be opened.
enum ConnectError {
    /// The keyboard is not there: nothing to log.
    Absent,
    /// Another program was talking to the keyboard.
    Foreign,
    /// Anything else; the text is logged once per distinct message.
    Failed(String),
}

enum Outcome {
    Stopped,
    Lost(String),
    /// The module was switched off: the session is closed.
    Disabled,
}

/// How a receiver session ended.
enum ReceiverEnd {
    Stopped,
    /// Scan again after this pause.
    Retry(Duration),
}

pub struct KeychronWorker {
    pub app: tauri::AppHandle,
    pub state: Arc<Mutex<ManagerState>>,
    pub commands: Receiver<KeychronCommand>,
    pub shutdown: Arc<AtomicBool>,
    /// Up while a write sequence runs; shutdown waits for it.
    pub write_in_flight: Arc<AtomicBool>,
    /// App config dir; the cache of the last USB read lives under it.
    pub config_dir: Option<PathBuf>,
}

impl KeychronWorker {
    pub fn run(mut self) {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| self.run_loop()));
        if outcome.is_err() {
            eprintln!("myhub: the keyboard worker panicked");
            // Nobody serves the keyboard any more: show it as disconnected.
            let cached = lock(&self.state).find(DEVICE_ID);
            if let Some(mut detail) = cached {
                detail.summary.connection = Connection::Disconnected;
                detail.read_only = true;
                if let Some(keyboard) = detail.keyboard.as_mut() {
                    keyboard.other_configurator = false;
                    keyboard.configurable = false;
                }
                self.publish(detail, false);
            }
        }
    }

    fn stopping(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    fn publish(&self, detail: DeviceDetail, force: bool) {
        publish_slot(
            &self.app,
            &self.state,
            self.stopping(),
            Slot::Keychron,
            Some(detail),
            force,
        );
    }

    fn run_loop(&mut self) {
        let mut last_error: Option<String> = None;
        let mut cache = self
            .config_dir
            .as_deref()
            .and_then(|dir| KeyboardCache::load(&cache_path(dir)));
        // Consecutive connect failures or short sessions; drives the back off.
        let mut failures: u32 = 0;
        while !self.stopping() {
            let mut pause = RECONNECT_PAUSE;
            // One enumeration per cycle answers both questions; none with the module off.
            let Some(found) = scan_if_enabled(self.module_on(), scan) else {
                last_error = None;
                if lock(&self.state).find(DEVICE_ID).is_some() {
                    publish_slot(
                        &self.app,
                        &self.state,
                        self.stopping(),
                        Slot::Keychron,
                        None,
                        true,
                    );
                }
                self.wait_while_off();
                continue;
            };
            if !found.keyboard && !found.receiver {
                last_error = None;
                self.wait(hotplug::scan_interval(IDLE_SCAN_INTERVAL));
                continue;
            }
            // USB has priority: the receiver is only looked at while the cable is out.
            if !found.keyboard {
                match self.serve_receiver(cache.as_ref(), &mut last_error) {
                    ReceiverEnd::Stopped => return,
                    ReceiverEnd::Retry(wait) => pause = wait,
                }
                self.wait(pause);
                continue;
            }
            match self.connect() {
                Ok(mut keyboard) => {
                    last_error = None;
                    eprintln!("myhub: keyboard connected");
                    let mut stamp = clock_ms();
                    self.store_cache(&mut cache, &keyboard.snapshot);
                    self.publish(self.stamped(&keyboard, true, stamp), true);
                    let started = Instant::now();
                    let outcome = self.serve(&mut keyboard, &mut stamp, &mut cache);
                    // Closes the interface.
                    match outcome {
                        Outcome::Stopped => return,
                        Outcome::Disabled => pause = Duration::ZERO,
                        Outcome::Lost(reason) => {
                            eprintln!("myhub: keyboard link lost: {reason}");
                            self.publish(self.stamped(&keyboard, false, stamp), false);
                            // A session that lasted is a success; a quick loss backs off.
                            failures = if started.elapsed() >= HEALTHY_SESSION {
                                0
                            } else {
                                failures + 1
                            };
                            pause = connect_backoff(failures);
                        }
                    }
                }
                Err(ConnectError::Absent) => last_error = None,
                Err(ConnectError::Foreign) => {
                    if last_error.as_deref() != Some("foreign") {
                        eprintln!("myhub: keyboard busy: another program is talking to it");
                        last_error = Some("foreign".to_owned());
                    }
                    pause = FOREIGN_CONNECT_WAIT;
                }
                Err(ConnectError::Failed(message)) => {
                    if last_error.as_deref() != Some(message.as_str()) {
                        eprintln!("myhub: keyboard not connected: {message}");
                        last_error = Some(message);
                    }
                    failures += 1;
                    pause = connect_backoff(failures);
                }
            }
            // A hotplug wake means something changed: start the back off over.
            if self.wait(pause) {
                failures = 0;
            }
        }
    }

    /// Keeps the snapshot as the cache of the last USB read, in memory and on disk
    /// (atomic write; a failure logs one line without the path and is otherwise ignored).
    fn store_cache(&self, cache: &mut Option<KeyboardCache>, snapshot: &Snapshot) {
        let fresh = cache_of(snapshot, clock_ms());
        if let Some(dir) = self.config_dir.as_deref()
            && let Err(e) = fresh.save(&cache_path(dir))
        {
            eprintln!("myhub: keyboard cache not written: {:?}", e.kind());
        }
        *cache = Some(fresh);
    }

    /// Looks for the interface, opens it and reads the snapshot.
    fn connect(&self) -> Result<Keyboard<keychron_k2he::HidapiLink>, ConnectError> {
        // `Session::open` enumerates itself; no separate presence check.
        let mut session = Session::open().map_err(|e| match e {
            SessionError::NotFound => ConnectError::Absent,
            other => ConnectError::Failed(other.to_string()),
        })?;
        let snapshot = read_snapshot(&mut session).map_err(|e| match e {
            SessionError::ForeignTraffic
            | SessionError::BackingOff { .. }
            | SessionError::BadReport(_) => ConnectError::Foreign,
            other => ConnectError::Failed(other.to_string()),
        })?;
        Ok(Keyboard::new(session, snapshot))
    }

    /// Listens until shutdown or a lost link, publishing every change.
    fn serve(
        &self,
        keyboard: &mut Keyboard<keychron_k2he::HidapiLink>,
        stamp: &mut u64,
        cache: &mut Option<KeyboardCache>,
    ) -> Outcome {
        // No periodic enumeration here: `keyboard.step` listens every POLL, and a removed
        // device makes that read fail, which ends the session as `Outcome::Lost`.
        while !self.stopping() {
            let mut replies = Vec::new();
            let mut dirty = false;
            let mut selects = Vec::new();
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    KeychronCommand::Refresh { reply } => replies.push(reply),
                    KeychronCommand::Write {
                        op,
                        deadline,
                        reply,
                    } => selects.push(PendingWrite {
                        op,
                        deadline,
                        reply,
                    }),
                    KeychronCommand::SettingsChanged => dirty = true,
                    KeychronCommand::Wake => {}
                }
            }
            // Switched off since the last pass: queued writes are refused (a write that
            // already started has finished; they run one at a time in this thread).
            if !self.module_on() {
                for write in selects {
                    let _ = write.reply.send(Err("disconnected".to_owned()));
                }
                answer_all(replies, None);
                return Outcome::Disabled;
            }
            if let Err(reason) = self.run_writes(keyboard, stamp, cache, selects) {
                answer_all(replies, Some(self.stamped(keyboard, false, *stamp)));
                return Outcome::Lost(reason);
            }
            let now = Instant::now();
            match keyboard.step(now, !replies.is_empty(), POLL) {
                Ok(changed) => {
                    if changed {
                        *stamp = clock_ms();
                        if !keyboard.other_configurator() {
                            self.store_cache(cache, &keyboard.snapshot);
                        }
                    }
                    dirty |= changed;
                }
                Err(reason) => {
                    *stamp = clock_ms();
                    let detail = self.stamped(keyboard, false, *stamp);
                    answer_all(replies, Some(detail));
                    return Outcome::Lost(reason);
                }
            }
            if dirty || !replies.is_empty() {
                let detail = self.stamped(keyboard, true, *stamp);
                self.publish(detail.clone(), !replies.is_empty());
                answer_all(replies, Some(detail));
            }
        }
        Outcome::Stopped
    }

    /// Runs the queued keyboard writes one after the other. `allowDeviceWrites` is
    /// read once per command. A change is cached, stored and published before the
    /// answer goes out. `Err` with the reason when the link is gone: the command that
    /// met it gets its error code, the ones behind it `disconnected`.
    fn run_writes(
        &self,
        keyboard: &mut Keyboard<keychron_k2he::HidapiLink>,
        stamp: &mut u64,
        cache: &mut Option<KeyboardCache>,
        selects: Vec<PendingWrite>,
    ) -> Result<(), String> {
        // Shutdown waits while the flag is up. The flag is set before the stop check and
        // shutdown sets the stop flag before it reads the flag, so one sees the other.
        let mut in_flight = (!selects.is_empty()).then(|| FlashFlag::set(&self.write_in_flight));
        if in_flight.is_some() && self.stopping() {
            for write in selects {
                let _ = write.reply.send(Err("disconnected".to_owned()));
            }
            return Ok(());
        }
        drain_writes(
            keyboard,
            selects,
            || self.writes_allowed(),
            |keyboard, changed, publish| {
                if changed {
                    *stamp = clock_ms();
                    if !keyboard.other_configurator() {
                        self.store_cache(cache, &keyboard.snapshot);
                    }
                }
                let detail = self.stamped(keyboard, true, *stamp);
                if publish {
                    // The write is over: shutdown must not wait on a worker blocked in the
                    // tray. Re-armed for the writes still queued.
                    in_flight.take();
                    self.publish(detail.clone(), true);
                    in_flight = Some(FlashFlag::set(&self.write_in_flight));
                }
                detail
            },
        )
    }

    /// The keyboard through the receiver: opens the receiver session, reads B1..B3 once
    /// and then B2 every [`LINK_POLL`], until shutdown, a lost link, or the cable showing
    /// up (the session is closed on return either way).
    fn serve_receiver(
        &self,
        cache: Option<&KeyboardCache>,
        last_error: &mut Option<String>,
    ) -> ReceiverEnd {
        let mut session = match Session::open_receiver() {
            Ok(session) => session,
            Err(SessionError::ReceiverNotFound) => {
                return ReceiverEnd::Retry(hotplug::scan_interval(IDLE_SCAN_INTERVAL));
            }
            Err(other) => return self.receiver_failed(other.to_string(), last_error),
        };
        let first = match read_link(&mut session) {
            Ok(first) => first,
            Err(
                SessionError::ForeignTraffic
                | SessionError::BackingOff { .. }
                | SessionError::BadReport(_),
            ) => {
                let message = "receiver busy: another program is talking to it".to_owned();
                if last_error.as_deref() != Some(message.as_str()) {
                    eprintln!("myhub: {message}");
                    *last_error = Some(message);
                }
                return ReceiverEnd::Retry(FOREIGN_CONNECT_WAIT);
            }
            Err(other) => return self.receiver_failed(other.to_string(), last_error),
        };
        *last_error = None;
        let mut link = Link::new(session, &first, Instant::now());
        eprintln!("myhub: keyboard receiver opened, linked: {}", link.linked);
        let mut stamp = clock_ms();
        if let Some(detail) = self.receiver_detail(&link, cache, stamp) {
            self.publish(detail, true);
        }
        let mut cable = CableCheck::new(Instant::now());
        while !self.stopping() {
            let mut replies = Vec::new();
            let mut dirty = false;
            let mut woken = false;
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    KeychronCommand::Refresh { reply } => replies.push(reply),
                    KeychronCommand::Write { reply, .. } => {
                        let code = no_usb_refusal(self.writes_allowed(), link.linked);
                        let _ = reply.send(Err(code.to_owned()));
                    }
                    KeychronCommand::SettingsChanged => dirty = true,
                    // A HID arrival (or shutdown): check for the cable now.
                    KeychronCommand::Wake => woken = true,
                }
            }
            if !self.module_on() {
                answer_all(replies, None);
                return ReceiverEnd::Retry(Duration::ZERO);
            }
            let now = Instant::now();
            match link.step(now, !replies.is_empty(), POLL) {
                Ok(changed) => {
                    if changed {
                        stamp = clock_ms();
                    }
                    dirty |= changed;
                }
                Err(reason) => {
                    eprintln!("myhub: keyboard receiver lost: {reason}");
                    stamp = clock_ms();
                    answer_all(replies, self.mark_gone(&link, cache, stamp));
                    return ReceiverEnd::Retry(RECONNECT_PAUSE);
                }
            }
            if cable.due(now, !replies.is_empty() || woken) {
                let found = scan();
                if found.keyboard {
                    // The cable is in: USB takes over, the caller opens it right away.
                    answer_all(replies, lock(&self.state).find(DEVICE_ID));
                    return ReceiverEnd::Retry(Duration::ZERO);
                }
                if !found.receiver {
                    eprintln!("myhub: keyboard receiver unplugged");
                    stamp = clock_ms();
                    answer_all(replies, self.mark_gone(&link, cache, stamp));
                    return ReceiverEnd::Retry(RECONNECT_PAUSE);
                }
            }
            if dirty || !replies.is_empty() {
                let detail = self.receiver_detail(&link, cache, stamp);
                if let Some(detail) = &detail {
                    self.publish(detail.clone(), !replies.is_empty());
                }
                answer_all(replies, detail);
            }
        }
        ReceiverEnd::Stopped
    }

    fn receiver_failed(&self, message: String, last_error: &mut Option<String>) -> ReceiverEnd {
        if last_error.as_deref() != Some(message.as_str()) {
            eprintln!("myhub: keyboard receiver not usable: {message}");
            *last_error = Some(message);
        }
        self.mark_disconnected();
        ReceiverEnd::Retry(hotplug::scan_interval(IDLE_SCAN_INTERVAL))
    }

    /// The detail for the receiver state: connected when B2 says a K2 HE is linked,
    /// disconnected when it is not but the keyboard was published before, otherwise
    /// `None` (it stays absent).
    fn receiver_detail<L: RawLink>(
        &self,
        link: &Link<L>,
        cache: Option<&KeyboardCache>,
        at_ms: u64,
    ) -> Option<DeviceDetail> {
        if !link.linked && lock(&self.state).find(DEVICE_ID).is_none() {
            return None;
        }
        Some(self.finish(link.detail(cache, link.linked), link.linked, at_ms))
    }

    /// The receiver is gone: publish the keyboard as disconnected if it was shown.
    fn mark_gone<L: RawLink>(
        &self,
        link: &Link<L>,
        cache: Option<&KeyboardCache>,
        at_ms: u64,
    ) -> Option<DeviceDetail> {
        lock(&self.state).find(DEVICE_ID)?;
        let detail = self.finish(link.detail(cache, false), false, at_ms);
        self.publish(detail.clone(), false);
        Some(detail)
    }

    /// A receiver that cannot be used: a published, connected keyboard goes to
    /// disconnected (its last values stay).
    fn mark_disconnected(&self) {
        let cached = lock(&self.state).find(DEVICE_ID);
        if let Some(mut detail) = cached
            && detail.summary.connection == Connection::Connected
        {
            detail.summary.connection = Connection::Disconnected;
            detail.read_only = true;
            detail.updated_at_ms = clock_ms();
            if let Some(keyboard) = detail.keyboard.as_mut() {
                keyboard.other_configurator = false;
                keyboard.configurable = false;
            }
            self.publish(detail, false);
        }
    }

    fn module_on(&self) -> bool {
        self.app
            .try_state::<SettingsState>()
            .is_none_or(|s| s.modules().keychron)
    }

    /// Idle while the module is off: no enumeration. Ends when the switch is on again (a
    /// settings change) or on shutdown; a hotplug wake does not end it.
    fn wait_while_off(&self) {
        while !self.stopping() {
            match self.commands.recv_timeout(Duration::from_millis(250)) {
                Ok(KeychronCommand::Refresh { reply }) => {
                    let _ = reply.send(None);
                }
                Ok(KeychronCommand::Write { reply, .. }) => {
                    let _ = reply.send(Err("disconnected".to_owned()));
                }
                Ok(KeychronCommand::SettingsChanged) => {
                    if self.module_on() {
                        return;
                    }
                }
                Ok(KeychronCommand::Wake) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(250)),
            }
        }
    }

    fn writes_allowed(&self) -> bool {
        self.app
            .try_state::<SettingsState>()
            .is_some_and(|s| s.get().allow_device_writes)
    }

    /// The detail with the read time stamped on it and `readOnly` from the settings.
    fn stamped<L: RawLink>(
        &self,
        keyboard: &Keyboard<L>,
        connected: bool,
        at_ms: u64,
    ) -> DeviceDetail {
        self.finish(keyboard.detail(connected), connected, at_ms)
    }

    fn finish(&self, mut detail: DeviceDetail, connected: bool, at_ms: u64) -> DeviceDetail {
        detail.updated_at_ms = at_ms;
        detail.read_only = !(self.writes_allowed() && connected);
        detail
    }

    /// Sleeps up to `duration`. A refresh command is answered with the cached detail
    /// (there is no session to read from) and does not end the sleep; a wake (HID arrival,
    /// shutdown) does.
    fn wait(&self, duration: Duration) -> bool {
        wait_for_wake(
            &self.commands,
            duration,
            || self.stopping(),
            |command| match command {
                KeychronCommand::Refresh { reply } => {
                    let cached = lock(&self.state).find(DEVICE_ID);
                    let _ = reply.send(cached);
                    false
                }
                KeychronCommand::Write { reply, .. } => {
                    let code = no_usb_refusal(self.writes_allowed(), false);
                    let _ = reply.send(Err(code.to_owned()));
                    false
                }
                // Switched off: end the pause so the card goes at once. Otherwise
                // disconnected: readOnly is already true.
                KeychronCommand::SettingsChanged => !self.module_on(),
                KeychronCommand::Wake => false,
            },
        )
    }
}

/// The idle pause: waits up to `duration` in short slices, hands every command except a
/// wake to `on_command`, and returns early on a wake, on `stopping` or when the sender is
/// gone for good (then it keeps the pace).
fn wait_for_wake(
    commands: &Receiver<KeychronCommand>,
    duration: Duration,
    stopping: impl Fn() -> bool,
    mut on_command: impl FnMut(KeychronCommand) -> bool,
) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stopping() {
            return false;
        }
        let slice = left.min(Duration::from_millis(250));
        match commands.recv_timeout(slice) {
            Ok(KeychronCommand::Wake) => return true,
            Ok(command) => {
                if on_command(command) {
                    return false;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            // The manager is gone: nothing to wake us, keep the pace.
            Err(RecvTimeoutError::Disconnected) => thread::sleep(slice),
        }
    }
}

/// Runs the queued writes one after the other. `writes_allowed` is asked once per
/// command. `settle(keyboard, changed, publish)` is called after each command (stamp,
/// cache, and publish when asked) and returns the detail to answer with; it is called
/// before the answer is sent, and for a command that ended the link only when something
/// changed (without publishing). `Err` with the reason when the link is gone: that
/// command gets its error code, the ones behind it `disconnected`.
fn drain_writes<L: RawLink>(
    keyboard: &mut Keyboard<L>,
    selects: Vec<PendingWrite>,
    mut writes_allowed: impl FnMut() -> bool,
    mut settle: impl FnMut(&Keyboard<L>, bool, bool) -> DeviceDetail,
) -> Result<(), String> {
    let mut selects = selects.into_iter();
    while let Some(select) = selects.next() {
        let result = keyboard.write(select.op, writes_allowed(), select.deadline, Instant::now());
        if let Some(reason) = result.lost {
            if result.changed {
                settle(keyboard, true, false);
            }
            let code = result.answer.err().unwrap_or_default();
            let _ = select.reply.send(Err(code));
            for rest in selects {
                let _ = rest.reply.send(Err("disconnected".to_owned()));
            }
            return Err(reason);
        }
        let detail = settle(keyboard, result.changed, result.changed);
        let _ = select.reply.send(result.answer.map(|()| detail));
    }
    Ok(())
}

fn answer_all(replies: Vec<Sender<Option<DeviceDetail>>>, detail: Option<DeviceDetail>) {
    for reply in replies {
        let _ = reply.send(detail.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::mpsc;

    use keychron_k2he::{LinkError, READ_BUF_LEN, REPORT_LEN};
    use keychron_proto::he::{HeInfo, ProfilesInfo};
    use keychron_proto::system::KcVersion;

    #[test]
    fn a_wake_ends_the_idle_pause_and_a_refresh_does_not() {
        let (tx, rx) = mpsc::channel();
        let (reply, answer) = mpsc::channel();
        tx.send(KeychronCommand::Refresh { reply }).unwrap();
        tx.send(KeychronCommand::Wake).unwrap();
        let mut handled = 0;
        let started = Instant::now();
        wait_for_wake(
            &rx,
            Duration::from_secs(30),
            || false,
            |command| {
                handled += 1;
                if let KeychronCommand::Refresh { reply } = command {
                    let _ = reply.send(None);
                }
                false
            },
        );
        // The refresh was served first, then the wake cut the 30 s pause short.
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(handled, 1);
        assert_eq!(answer.try_recv().unwrap(), None);
    }

    #[test]
    fn the_idle_pause_runs_out_without_a_wake_and_stops_on_shutdown() {
        let (_tx, rx) = mpsc::channel();
        let started = Instant::now();
        wait_for_wake(&rx, Duration::from_millis(300), || false, |_| false);
        assert!(started.elapsed() >= Duration::from_millis(300));
        let started = Instant::now();
        wait_for_wake(&rx, Duration::from_secs(30), || true, |_| false);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_forced_cable_check_does_not_wait_for_the_slow_interval() {
        let t0 = Instant::now();
        let mut cable = CableCheck::new(t0);
        // A wake (HID arrival) forces the check like a refresh does, and re-arms it.
        assert!(cable.due(t0 + Duration::from_secs(1), true));
        assert!(!cable.due(t0 + Duration::from_secs(2), false));
    }

    #[test]
    fn cable_check_is_slow_and_a_refresh_forces_it() {
        let t0 = Instant::now();
        let mut cable = CableCheck::new(t0);
        // The old 2 s cadence is gone.
        assert!(!cable.due(t0 + Duration::from_secs(2), false));
        assert!(!cable.due(t0 + CABLE_CHECK_INTERVAL - Duration::from_millis(1), false));
        assert!(cable.due(t0 + CABLE_CHECK_INTERVAL, false));
        // Re-armed from the check.
        let t1 = t0 + CABLE_CHECK_INTERVAL;
        assert!(!cable.due(t1 + Duration::from_secs(2), false));
        // A refresh checks at once and re-arms too.
        assert!(cable.due(t1 + Duration::from_secs(3), true));
        assert!(!cable.due(t1 + Duration::from_secs(4), false));
    }

    use super::*;

    #[derive(Default)]
    struct Shared {
        inbox: VecDeque<Vec<u8>>,
        written: usize,
        /// Frame bytes of every report written.
        sent: Vec<u8>,
        /// The reply to a B2 request; other requests are never answered.
        b2: Option<Vec<u8>>,
        /// Command, sub and first argument of every report written.
        frames: Vec<[u8; 3]>,
        /// A keyboard that answers A9 10 and A9 11 when set.
        he: Option<He>,
        /// The write that fails with a link error (1-based count of reports written).
        fail_write_at: Option<usize>,
    }

    /// A keyboard's HE profile state for the select tests.
    #[derive(Default)]
    struct He {
        current: u8,
        /// A9 11 gets no reply and changes nothing.
        mute_select: bool,
        /// A9 11 is answered but the profile does not change.
        stuck: bool,
        /// A9 11 is answered by a report that is not its reply (another program).
        foreign_select: bool,
        /// Global word of every profile (mode | distance << 2 | press << 8 | release << 14).
        word: u32,
        /// [brightness, effect, speed, hue, saturation]
        light: [u8; 5],
        /// A9 14 gets no reply and changes nothing.
        mute_travel: bool,
        /// A9 1F changes the word to this one (the save did not keep the value).
        corrupt_on_save: Option<u32>,
        /// Brightness is stored this much lower than sent.
        bright_delta: u8,
    }

    /// A link that holds what the test queued and counts the reports written; it
    /// never answers a request.
    struct Mock(Rc<RefCell<Shared>>);

    impl RawLink for Mock {
        fn write_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), LinkError> {
            let mut shared = self.0.borrow_mut();
            shared.written += 1;
            if shared.fail_write_at == Some(shared.written) {
                return Err(LinkError::Hid("unplugged".to_owned()));
            }
            shared.sent.push(report[1]);
            shared.frames.push([report[1], report[2], report[3]]);
            if report[1] == 0xA9
                && let Some(he) = shared.he.as_mut()
            {
                let reply = match report[2] {
                    0x10 => Some(vec![0xA9, 0x10, he.current, 3, 0x5C, 0x03, 20, 20]),
                    0x11 if he.foreign_select => Some(vec![0x55; 32]),
                    0x11 if !he.mute_select => {
                        if !he.stuck {
                            he.current = report[3];
                        }
                        Some(vec![0xA9, 0x11, 0, report[3]])
                    }
                    0x12 => {
                        let mut r = vec![0xA9, 0x12, report[3], 0, 0, 4];
                        r.extend_from_slice(&he.word.to_le_bytes());
                        Some(r)
                    }
                    0x14 if !he.mute_travel => {
                        let p = |i: usize| u32::from(report[i]);
                        he.word = p(4) | p(5) << 2 | p(6) << 8 | p(7) << 14;
                        Some(vec![0xA9, 0x14, 0])
                    }
                    0x1F => {
                        if let Some(w) = he.corrupt_on_save {
                            he.word = w;
                        }
                        Some(vec![0xA9, 0x1F, 0])
                    }
                    _ => None,
                };
                if let Some(mut reply) = reply {
                    reply.resize(32, 0);
                    shared.inbox.push_back(reply);
                }
            }
            if matches!(report[1], 0x07..=0x09)
                && report[2] == 0x03
                && let Some(he) = shared.he.as_mut()
            {
                let id = usize::from(report[3]);
                let mut reply = match report[1] {
                    0x08 => {
                        let mut r = vec![0x08, 0x03, report[3]];
                        match id {
                            1..=3 => r.push(he.light[id - 1]),
                            _ => r.extend_from_slice(&he.light[3..5]),
                        }
                        r
                    }
                    0x07 => {
                        match id {
                            1 => he.light[0] = report[4].saturating_sub(he.bright_delta),
                            2 | 3 => he.light[id - 1] = report[4],
                            _ => he.light[3..5].copy_from_slice(&report[4..6]),
                        }
                        report[1..].to_vec()
                    }
                    _ => report[1..].to_vec(),
                };
                reply.resize(32, 0);
                shared.inbox.push_back(reply);
            }
            if report[1] == 0xB2
                && let Some(reply) = shared.b2.clone()
            {
                shared.inbox.push_back(reply);
            }
            Ok(())
        }

        fn read_report(
            &mut self,
            buf: &mut [u8; READ_BUF_LEN],
            _timeout_ms: i32,
        ) -> Result<usize, LinkError> {
            match self.0.borrow_mut().inbox.pop_front() {
                Some(r) => {
                    buf[..r.len()].copy_from_slice(&r);
                    Ok(r.len())
                }
                None => Ok(0),
            }
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            via_protocol: 12,
            kc: KcVersion {
                kc_version: 2,
                instruction_set: 2,
            },
            firmware: Some("v1.2.1".to_owned()),
            features: None,
            layer: None,
            misc: None,
            layout_lang: None,
            timers: None,
            layer_count: Some(4),
            macro_count: None,
            macro_buffer_size: None,
            light: keychron_k2he::LightSettings::default(),
            lighting: keychron_k2he::Lighting::default(),
            he: HeInfo {
                version: 4,
                feature_byte: 0,
                switch_axis: false,
                reset_single_key: false,
                dkr: false,
                turbo: false,
                gamepad_disable: false,
                support_mechanical: false,
            },
            profiles_info: ProfilesInfo {
                current: 1,
                count: 3,
                size: 860,
                dks_count: 20,
                snap_count: 20,
            },
            profiles: Vec::new(),
            curve: None,
            keymaps: None,
            elapsed_ms: 0,
            requests: 0,
            notes: Vec::new(),
        }
    }

    fn keyboard() -> (Keyboard<Mock>, Rc<RefCell<Shared>>) {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let mut session = Session::with_link(Mock(Rc::clone(&shared)));
        session.set_reply_timeout(Duration::from_millis(200));
        (Keyboard::new(session, snapshot()), shared)
    }

    #[test]
    fn foreign_traffic_sets_the_flag_and_nothing_is_sent_until_the_quiet_time_passed() {
        let (mut kb, shared) = keyboard();
        let t0 = Instant::now();
        // A report that is neither a reply nor a known event.
        shared.borrow_mut().inbox.push_back(vec![0x55; 32]);
        assert_eq!(kb.step(t0, false, Duration::ZERO), Ok(true));
        assert!(kb.other_configurator());
        assert!(kb.detail(true).keyboard.unwrap().other_configurator);
        // A refresh while another program talks answers the cache and sends nothing.
        assert_eq!(kb.step(t0, true, Duration::ZERO), Ok(false));
        let just_before = t0 + FOREIGN_QUIET - Duration::from_millis(1);
        assert_eq!(kb.step(just_before, true, Duration::ZERO), Ok(false));
        assert_eq!(shared.borrow().written, 0);
        assert!(kb.other_configurator());
        // More foreign traffic restarts the quiet time.
        shared.borrow_mut().inbox.push_back(vec![0x66; 32]);
        assert_eq!(kb.step(just_before, false, Duration::ZERO), Ok(false));
        let later = t0 + FOREIGN_QUIET + Duration::from_secs(1);
        assert_eq!(kb.step(later, false, Duration::ZERO), Ok(false));
        assert_eq!(shared.borrow().written, 0);
        // After the quiet time the worker tries the full read. The session own back-off
        // runs on the real clock (5 s, started by the foreign reports above), so here the
        // read is refused before a byte is written: the flag stays and another quiet time
        // starts, which shows the attempt was made.
        let quiet = just_before + FOREIGN_QUIET;
        assert_eq!(kb.step(quiet, false, Duration::ZERO), Ok(false));
        assert_eq!(kb.last_foreign, Some(quiet));
        assert_eq!(shared.borrow().written, 0);
        assert!(kb.other_configurator());
    }

    #[test]
    fn events_update_the_snapshot_without_sending_and_a_clean_listen_changes_nothing() {
        let (mut kb, shared) = keyboard();
        let now = Instant::now();
        assert_eq!(kb.step(now, false, Duration::ZERO), Ok(false));
        let mut event = vec![0xA9, 0x11, 0x00, 0x02];
        event.resize(32, 0);
        shared.borrow_mut().inbox.push_back(event);
        assert_eq!(kb.step(now, false, Duration::ZERO), Ok(true));
        let he = kb.detail(true).keyboard.unwrap().he.unwrap();
        assert_eq!(he.current_profile, 2);
        assert!(!kb.other_configurator());
        assert_eq!(shared.borrow().written, 0);
    }

    const B2_UP: [u8; 7] = [0xB2, 0x01, 0x34, 0x34, 0x20, 0x0E, 0x01];
    const B2_NO_KEYBOARD: [u8; 7] = [0xB2, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
    const B2_NOT_CONNECTED: [u8; 7] = [0xB2, 0x01, 0x34, 0x34, 0x20, 0x0E, 0x00];

    fn frame(bytes: &[u8]) -> Vec<u8> {
        let mut v = bytes.to_vec();
        v.resize(32, 0);
        v
    }

    fn link(linked: bool, b2: Option<&[u8]>, now: Instant) -> (Link<Mock>, Rc<RefCell<Shared>>) {
        let shared = Rc::new(RefCell::new(Shared::default()));
        shared.borrow_mut().b2 = b2.map(frame);
        let mut session =
            Session::with_link_for(Mock(Rc::clone(&shared)), keychron_proto::Target::Receiver);
        session.set_reply_timeout(Duration::from_millis(200));
        let link = Link {
            session,
            firmware: Some("d.3.1".to_owned()),
            linked,
            last_foreign: None,
            next_read: now + LINK_POLL,
            read_failing: false,
        };
        (link, shared)
    }

    #[test]
    fn receiver_reads_b2_only_when_due_and_follows_the_link_state() {
        let t0 = Instant::now();
        let (mut rx, shared) = link(false, Some(&B2_UP), t0);
        // Not due yet: nothing is sent.
        assert_eq!(rx.step(t0, false, Duration::ZERO), Ok(false));
        assert_eq!(shared.borrow().written, 0);
        // A refresh reads B2 now: a connected K2 HE.
        assert_eq!(rx.step(t0, true, Duration::ZERO), Ok(true));
        assert!(rx.linked);
        assert_eq!(shared.borrow().sent, vec![0xB2]);
        // The same answer again changes nothing; the poll interval holds the next read.
        assert_eq!(rx.step(t0, true, Duration::ZERO), Ok(false));
        assert_eq!(
            rx.step(t0 + LINK_POLL / 2, false, Duration::ZERO),
            Ok(false)
        );
        assert_eq!(shared.borrow().sent, vec![0xB2, 0xB2]);
        // Due again: the keyboard went away (B2 connected byte 0).
        shared.borrow_mut().b2 = Some(frame(&B2_NOT_CONNECTED));
        assert_eq!(rx.step(t0 + LINK_POLL, false, Duration::ZERO), Ok(true));
        assert!(!rx.linked);
        // Another device behind the receiver counts as not linked.
        let (mut rx, _) = link(true, Some(&B2_NO_KEYBOARD), t0);
        assert_eq!(rx.step(t0, true, Duration::ZERO), Ok(true));
        assert!(!rx.linked);
    }

    #[test]
    fn receiver_foreign_traffic_stops_sending_until_the_quiet_time_passed() {
        let t0 = Instant::now();
        let (mut rx, shared) = link(true, Some(&B2_UP), t0);
        shared.borrow_mut().inbox.push_back(frame(&[0x55; 32]));
        assert_eq!(rx.step(t0, false, Duration::ZERO), Ok(true));
        assert!(rx.other_configurator());
        assert!(rx.detail(None, true).keyboard.unwrap().other_configurator);
        // Neither a refresh nor the poll interval sends anything meanwhile.
        assert_eq!(rx.step(t0, true, Duration::ZERO), Ok(false));
        let later = t0 + FOREIGN_QUIET - Duration::from_secs(1);
        assert_eq!(rx.step(later, false, Duration::ZERO), Ok(false));
        assert_eq!(shared.borrow().written, 0);
        // After the quiet time the flag clears and B2 is due. The session own back-off runs
        // on the real clock (5 s from the foreign report above), so the read is refused
        // before a byte is written and a new quiet time starts: the attempt was made.
        let quiet = t0 + LINK_POLL.max(FOREIGN_QUIET);
        assert_eq!(rx.step(quiet, false, Duration::ZERO), Ok(false));
        assert_eq!(rx.last_foreign, Some(quiet));
        assert_eq!(shared.borrow().written, 0);
    }

    #[test]
    fn receiver_detail_is_connected_or_disconnected_and_never_configurable() {
        let t0 = Instant::now();
        let (rx, _) = link(true, None, t0);
        let up = rx.detail(None, true);
        assert_eq!(up.summary.connection, Connection::Connected);
        assert_eq!(up.transport, "2.4 GHz (Keychron Link)");
        assert!(!up.keyboard.unwrap().configurable);
        let down = rx.detail(None, false);
        assert_eq!(down.summary.connection, Connection::Disconnected);
    }

    const SELECT_BUDGET: Duration = Duration::from_secs(13);

    /// A keyboard on profile 1 (0-based) that answers A9 10 and A9 11.
    fn he_keyboard(mute_select: bool, stuck: bool) -> (Keyboard<Mock>, Rc<RefCell<Shared>>) {
        let (kb, shared) = keyboard();
        shared.borrow_mut().he = Some(He {
            current: 1,
            mute_select,
            stuck,
            foreign_select: false,
            ..He::default()
        });
        (kb, shared)
    }

    fn select(kb: &mut Keyboard<Mock>, writes: bool, profile: i64) -> WriteResult {
        let now = Instant::now();
        kb.select_profile(writes, profile, now + SELECT_BUDGET, now)
    }

    #[test]
    fn select_preconditions_refuse_before_any_frame() {
        let (mut kb, shared) = he_keyboard(false, false);
        let err = |r: WriteResult| r.answer.unwrap_err();
        assert_eq!(err(select(&mut kb, false, 2)), "writesDisabled");
        assert_eq!(err(select(&mut kb, true, 3)), "outOfRange");
        assert_eq!(err(select(&mut kb, true, -1)), "outOfRange");
        let now = Instant::now();
        let late = kb.select_profile(true, 2, now + Duration::from_secs(10), now);
        assert_eq!(err(late), "deviceError:budget");
        kb.mark_foreign(now);
        assert_eq!(err(select(&mut kb, true, 2)), "busy");
        // Writes off outranks busy.
        assert_eq!(err(select(&mut kb, false, 2)), "writesDisabled");
        assert_eq!(shared.borrow().written, 0);
    }

    #[test]
    fn no_usb_refusal_orders_writes_off_then_receiver_then_disconnected() {
        // Pure function: the receiver loop and `wait()` only call it and send no frame.
        assert_eq!(no_usb_refusal(true, true), "usbRequired");
        assert_eq!(no_usb_refusal(false, true), "writesDisabled");
        assert_eq!(no_usb_refusal(false, false), "writesDisabled");
        assert_eq!(no_usb_refusal(true, false), "disconnected");
    }

    #[test]
    fn select_sends_a9_10_a9_11_a9_10_and_shows_the_new_profile() {
        let (mut kb, shared) = he_keyboard(false, false);
        let result = select(&mut kb, true, 2);
        assert!(result.answer.is_ok() && result.changed && result.lost.is_none());
        assert_eq!(
            shared.borrow().frames,
            vec![[0xA9, 0x10, 0], [0xA9, 0x11, 2], [0xA9, 0x10, 0]]
        );
        let he = kb.detail(true).keyboard.unwrap().he.unwrap();
        assert_eq!(he.current_profile, 2);
        // The profile is already current: nothing is written.
        let again = select(&mut kb, true, 2);
        assert!(again.answer.is_ok() && !again.changed);
        assert_eq!(shared.borrow().frames.len(), 4);
    }

    #[test]
    fn select_reports_verify_failed_with_the_profile_the_keyboard_has() {
        let (mut kb, shared) = he_keyboard(false, true);
        let result = select(&mut kb, true, 2);
        assert_eq!(result.answer.unwrap_err(), "verifyFailed");
        assert_eq!(
            kb.detail(true)
                .keyboard
                .unwrap()
                .he
                .unwrap()
                .current_profile,
            1
        );
        assert_eq!(shared.borrow().frames.len(), 3);
    }

    #[test]
    fn select_timeout_rereads_once_and_never_resends_a9_11() {
        let (mut kb, shared) = he_keyboard(true, false);
        let result = select(&mut kb, true, 2);
        assert_eq!(result.answer.unwrap_err(), "deviceError:timeout");
        assert!(result.lost.is_none());
        assert_eq!(
            shared.borrow().frames,
            vec![[0xA9, 0x10, 0], [0xA9, 0x11, 2], [0xA9, 0x10, 0]]
        );
        assert_eq!(
            kb.detail(true)
                .keyboard
                .unwrap()
                .he
                .unwrap()
                .current_profile,
            1
        );
    }

    fn pending(profile: i64) -> (PendingWrite, mpsc::Receiver<Result<DeviceDetail, String>>) {
        let (reply, answer) = mpsc::channel();
        let deadline = Instant::now() + SELECT_BUDGET;
        (
            PendingWrite {
                op: KeyboardWrite::Profile(profile),
                deadline,
                reply,
            },
            answer,
        )
    }

    fn settle_detail(kb: &Keyboard<Mock>) -> DeviceDetail {
        kb.detail(true)
    }

    #[test]
    fn a_foreign_report_during_select_answers_busy_without_a_re_read() {
        let (mut kb, shared) = he_keyboard(false, false);
        shared.borrow_mut().he.as_mut().unwrap().foreign_select = true;
        let result = select(&mut kb, true, 2);
        assert_eq!(result.answer.unwrap_err(), "busy");
        assert!(result.lost.is_none());
        assert!(kb.other_configurator());
        // A9 10, A9 11 and nothing after.
        assert_eq!(
            shared.borrow().frames,
            vec![[0xA9, 0x10, 0], [0xA9, 0x11, 2]]
        );
    }

    #[test]
    fn link_loss_mid_select_answers_its_code_and_the_queued_ones_disconnected() {
        let (mut kb, shared) = he_keyboard(false, false);
        // The second report written (A9 11) fails.
        shared.borrow_mut().fail_write_at = Some(2);
        let (first, first_rx) = pending(2);
        let (second, second_rx) = pending(0);
        let result = drain_writes(
            &mut kb,
            vec![first, second],
            || true,
            |kb, _, _| settle_detail(kb),
        );
        assert!(result.is_err());
        assert_eq!(
            first_rx.try_recv().unwrap().unwrap_err(),
            "deviceError:link"
        );
        assert_eq!(second_rx.try_recv().unwrap().unwrap_err(), "disconnected");
        // A9 10, the failed A9 11, no re-read.
        assert_eq!(shared.borrow().written, 2);
    }

    #[test]
    fn connect_failures_back_off_2s_5s_then_the_idle_interval() {
        assert_eq!(connect_backoff(1), Duration::from_secs(2));
        assert_eq!(connect_backoff(2), Duration::from_secs(5));
        assert_eq!(
            connect_backoff(3),
            hotplug::scan_interval(IDLE_SCAN_INTERVAL)
        );
        assert_eq!(connect_backoff(40), connect_backoff(3));
    }

    #[test]
    fn a_switched_off_module_enumerates_nothing() {
        let calls = std::cell::Cell::new(0);
        let enumerate = || {
            calls.set(calls.get() + 1);
            Presence {
                keyboard: true,
                receiver: true,
            }
        };
        assert!(scan_if_enabled(false, enumerate).is_none());
        assert_eq!(calls.get(), 0);
        assert!(scan_if_enabled(true, enumerate).is_some());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn publish_comes_before_the_answer_and_writes_are_checked_per_command() {
        let (mut kb, shared) = he_keyboard(false, false);
        let (first, first_rx) = pending(2);
        let (second, second_rx) = pending(0);
        let published = RefCell::new(Vec::new());
        let asked = RefCell::new(0);
        let result = drain_writes(
            &mut kb,
            vec![first, second],
            || {
                // The setting flips between the two commands.
                *asked.borrow_mut() += 1;
                *asked.borrow() == 1
            },
            |kb, changed, publish| {
                if publish {
                    // Nobody has been answered yet.
                    assert!(first_rx.try_recv().is_err());
                    published.borrow_mut().push(changed);
                }
                settle_detail(kb)
            },
        );
        assert!(result.is_ok());
        assert_eq!(*published.borrow(), vec![true]);
        let detail = first_rx.try_recv().unwrap().unwrap();
        assert_eq!(detail.keyboard.unwrap().he.unwrap().current_profile, 2);
        assert_eq!(second_rx.try_recv().unwrap().unwrap_err(), "writesDisabled");
        // Only the first command sent frames: A9 10, A9 11, A9 10.
        assert_eq!(shared.borrow().written, 3);
        assert_eq!(*asked.borrow(), 2);
    }

    fn word(mode: u32, distance: u32, press: u32, release: u32) -> u32 {
        mode | distance << 2 | press << 8 | release << 14
    }

    /// A keyboard on profile 1 with global words and lighting set, and a snapshot that
    /// holds the same values (three profiles, all rapid trigger 2.0 / 0.4 / 0.4).
    fn settings_keyboard(configure: impl FnOnce(&mut He)) -> (Keyboard<Mock>, Rc<RefCell<Shared>>) {
        let (mut kb, shared) = he_keyboard(false, false);
        {
            let mut s = shared.borrow_mut();
            let he = s.he.as_mut().unwrap();
            he.word = word(2, 20, 4, 4);
            he.light = [200, 6, 50, 10, 20];
            configure(he);
        }
        kb.snapshot.profiles = (0..3)
            .map(|_| keychron_proto::he::ProfileBuffer {
                rows: 6,
                cols: 16,
                global: KeyWord::from_u32(word(2, 20, 4, 4)),
                keys: Vec::new(),
                dks: Vec::new(),
                snap: Vec::new(),
                name: String::new(),
            })
            .collect();
        kb.snapshot.light = keychron_k2he::LightSettings {
            brightness: Some(200),
            effect: Some(6),
            speed: Some(50),
            hue: Some(10),
            saturation: Some(20),
        };
        (kb, shared)
    }

    fn travel(mode: TravelModeArg, actuation: f64, press: f64, release: f64) -> KeyboardWrite {
        KeyboardWrite::travel(1, mode, actuation, press, release).unwrap()
    }

    fn run(kb: &mut Keyboard<Mock>, writes: bool, op: KeyboardWrite) -> WriteResult {
        let now = Instant::now();
        kb.write(op, writes, now + Duration::from_secs(23), now)
    }

    fn profile_view(kb: &Keyboard<Mock>, index: usize) -> myhub_core::HeProfileView {
        kb.detail(true).keyboard.unwrap().he.unwrap().profiles[index].clone()
    }

    #[test]
    fn travel_sends_the_exact_frames_once_and_updates_the_current_profile() {
        let (mut kb, shared) = settings_keyboard(|_| {});
        let op = travel(TravelModeArg::RapidTrigger, 2.2, 0.5, 0.6);
        let result = run(&mut kb, true, op);
        assert!(result.answer.is_ok() && result.changed && result.lost.is_none());
        // A9 10, word, A9 14, word, A9 10 (profile check), A9 1F, word. Reads once each.
        assert_eq!(
            shared.borrow().frames,
            vec![
                [0xA9, 0x10, 0],
                [0xA9, 0x12, 1],
                [0xA9, 0x14, 1],
                [0xA9, 0x12, 1],
                [0xA9, 0x10, 0],
                [0xA9, 0x1F, 1],
                [0xA9, 0x12, 1],
            ]
        );
        let p = profile_view(&kb, 1);
        assert_eq!((p.actuation_mm, p.press_mm, p.release_mm), (2.2, 0.5, 0.6));
        // Only the current profile changed.
        assert_eq!(profile_view(&kb, 0).actuation_mm, 2.0);
    }

    #[test]
    fn lighting_sends_the_changed_fields_then_saves_and_shows_the_stored_values() {
        let (mut kb, shared) = settings_keyboard(|he| he.bright_delta = 1);
        let op = KeyboardWrite::lighting(Some(4), Some(120), None, None, None).unwrap();
        let result = run(&mut kb, true, op);
        assert!(result.answer.is_ok() && result.changed);
        let frames: Vec<[u8; 3]> = shared.borrow().frames.clone();
        let writes: Vec<[u8; 3]> = frames
            .iter()
            .copied()
            .filter(|f| matches!(f[0], 0x07 | 0x09))
            .collect();
        // Effect (id 2) first, then brightness (id 1), then the save.
        assert_eq!(
            writes,
            vec![[0x07, 0x03, 2], [0x07, 0x03, 1], [0x09, 0x03, 0]]
        );
        // Four reads before, four after the writes, four after the save.
        assert_eq!(frames.iter().filter(|f| f[0] == 0x08).count(), 12);
        let light = kb.detail(true).keyboard.unwrap().lighting.unwrap();
        // The keyboard stores brightness one lower than sent.
        assert_eq!((light.effect, light.brightness), (4, 119));
    }

    #[test]
    fn writes_off_send_nothing_for_travel_and_lighting() {
        let (mut kb, shared) = settings_keyboard(|_| {});
        let travel_op = travel(TravelModeArg::Static, 3.0, 0.0, 0.0);
        let light_op = KeyboardWrite::lighting(None, None, Some(10), None, None).unwrap();
        for op in [travel_op, light_op] {
            assert_eq!(
                run(&mut kb, false, op).answer.unwrap_err(),
                "writesDisabled"
            );
        }
        kb.mark_foreign(Instant::now());
        assert_eq!(run(&mut kb, true, travel_op).answer.unwrap_err(), "busy");
        let now = Instant::now();
        let late = kb.write(light_op, true, now + Duration::from_secs(19), now);
        assert_eq!(late.answer.unwrap_err(), "busy");
        assert_eq!(shared.borrow().written, 0);
    }

    #[test]
    fn a_short_deadline_refuses_with_budget_before_any_frame() {
        let (mut kb, shared) = settings_keyboard(|_| {});
        let light_op = KeyboardWrite::lighting(None, None, Some(10), None, None).unwrap();
        let now = Instant::now();
        let late = kb.write(light_op, true, now + Duration::from_secs(19), now);
        assert_eq!(late.answer.unwrap_err(), "deviceError:budget");
        assert_eq!(shared.borrow().written, 0);
    }

    #[test]
    fn a_save_that_did_not_keep_the_value_answers_verify_failed_saved_and_re_reads() {
        let corrupt = word(2, 30, 4, 4);
        let (mut kb, shared) = settings_keyboard(|he| he.corrupt_on_save = Some(corrupt));
        let result = run(
            &mut kb,
            true,
            travel(TravelModeArg::RapidTrigger, 2.2, 0.5, 0.6),
        );
        assert_eq!(result.answer.unwrap_err(), "verifyFailed:saved");
        assert!(result.changed && result.lost.is_none());
        // The snapshot shows what the keyboard holds after the re-read.
        assert_eq!(profile_view(&kb, 1).actuation_mm, 3.0);
        assert_eq!(shared.borrow().frames.last(), Some(&[0xA9, 0x12, 1]));
    }

    #[test]
    fn a_travel_write_that_times_out_is_not_resent_and_the_state_is_read_again() {
        let (mut kb, shared) = settings_keyboard(|he| he.mute_travel = true);
        let result = run(
            &mut kb,
            true,
            travel(TravelModeArg::RapidTrigger, 2.2, 0.5, 0.6),
        );
        assert_eq!(result.answer.unwrap_err(), "deviceError:timeout");
        assert!(result.lost.is_none());
        let frames = shared.borrow().frames.clone();
        assert_eq!(frames.iter().filter(|f| f[..2] == [0xA9, 0x14]).count(), 1);
        // A9 10, word, the A9 14, then the re-read: A9 10 and the word.
        assert_eq!(
            frames,
            vec![
                [0xA9, 0x10, 0],
                [0xA9, 0x12, 1],
                [0xA9, 0x14, 1],
                [0xA9, 0x10, 0],
                [0xA9, 0x12, 1],
            ]
        );
        assert_eq!(profile_view(&kb, 1).actuation_mm, 2.0);
    }

    #[test]
    fn travel_and_lighting_values_are_range_checked_in_the_units_of_the_ui() {
        use TravelModeArg::{RapidTrigger, Static};
        let bad = |r: Result<KeyboardWrite, String>| r.unwrap_err();
        assert!(KeyboardWrite::travel(1, Static, 0.2, 0.0, 0.0).is_ok());
        assert!(KeyboardWrite::travel(1, RapidTrigger, 3.8, 0.1, 3.8).is_ok());
        for actuation in [0.1, 3.9, f64::NAN] {
            assert_eq!(
                bad(KeyboardWrite::travel(1, Static, actuation, 0.5, 0.5)),
                "outOfRange"
            );
        }
        assert_eq!(
            bad(KeyboardWrite::travel(1, RapidTrigger, 2.0, 0.0, 0.5)),
            "outOfRange"
        );
        assert_eq!(
            bad(KeyboardWrite::travel(1, RapidTrigger, 2.0, 0.5, 3.9)),
            "outOfRange"
        );
        // 0.1 mm units, rounded.
        let Ok(KeyboardWrite::Travel { settings: t, .. }) =
            KeyboardWrite::travel(1, RapidTrigger, 2.04, 0.46, 0.5)
        else {
            panic!("valid")
        };
        assert_eq!((t.actuation, t.press, t.release), (20, 5, 5));
        assert!(KeyboardWrite::lighting(Some(24), Some(255), Some(0), Some(0), Some(255)).is_ok());
        for (e, b, s, h, sat) in [
            (Some(25), None, None, None, None),
            (Some(-1), None, None, None, None),
            (None, Some(256), None, None, None),
            (None, None, Some(-1), None, None),
            (None, None, None, Some(10), None),
            (None, None, None, None, Some(10)),
            (None, None, None, Some(256), Some(0)),
            (None, None, None, None, None),
        ] {
            assert_eq!(bad(KeyboardWrite::lighting(e, b, s, h, sat)), "outOfRange");
        }
    }

    #[test]
    fn a_travel_write_for_another_profile_is_refused_before_any_write_frame() {
        let (mut kb, shared) = settings_keyboard(|_| {});
        // The UI shows profile 0 as active, the keyboard is on 1.
        let op = KeyboardWrite::travel(0, TravelModeArg::RapidTrigger, 2.2, 0.5, 0.6).unwrap();
        let result = run(&mut kb, true, op);
        assert_eq!(result.answer.unwrap_err(), "profileChanged");
        let frames = shared.borrow().frames.clone();
        assert!(
            frames
                .iter()
                .all(|f| f[0] == 0xA9 && matches!(f[1], 0x10 | 0x12))
        );
        assert_eq!(frames[0], [0xA9, 0x10, 0]);
        // Nothing was written; the cache still shows profile 1 as current.
        assert_eq!(
            kb.detail(true)
                .keyboard
                .unwrap()
                .he
                .unwrap()
                .current_profile,
            1
        );
    }

    #[test]
    fn values_already_held_send_no_write_and_no_save() {
        let (mut kb, shared) = settings_keyboard(|_| {});
        // The keyboard holds rapid trigger 2.0 / 0.4 / 0.4 and effect 6, brightness 200.
        let same = travel(TravelModeArg::RapidTrigger, 2.0, 0.4, 0.4);
        assert!(run(&mut kb, true, same).answer.is_ok());
        let light = KeyboardWrite::lighting(Some(6), Some(200), None, None, None).unwrap();
        assert!(run(&mut kb, true, light).answer.is_ok());
        let frames = shared.borrow().frames.clone();
        assert!(
            frames
                .iter()
                .all(|f| !matches!((f[0], f[1]), (0xA9, 0x14 | 0x1F) | (0x07 | 0x09, _)))
        );
    }
}
