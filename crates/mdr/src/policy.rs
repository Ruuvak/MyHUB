//! Safety allowlist and the only way to build request bytes for the wire.
//!
//! A transport builds every outgoing frame with [`Policy::check`] (or
//! [`Policy::check_alert_answer`]), which returns a [`CheckedRequest`]. That type has
//! private fields, no public constructor and has no `Clone`, and its encoder is crate private: only
//! [`crate::link::Link::send`] turns it into wire bytes (it takes the request by value),
//! so the bytes that are sent are exactly the bytes that were checked. ACK frames are
//! the one other outgoing frame and carry no payload ([`crate::frame::encode_ack`]).
//!
//! Layers, evaluated in this order. Everything not listed is denied.
//!
//! 1. Hard deny, cannot be enabled by any tier:
//!    * data types other than `DATA_MDR` (0x0C) and `DATA_MDR_NO2` (0x0E): every
//!      `SHOT_*`, `LARGE_DATA_*`, `DATA_COMMON`, ACK and unknown type.
//!    * firmware update, table 1 `UPDT_*` commands 0x30..=0x39 (`UPDT_GET_CAPABILITY`
//!      0x30 up to `UPDT_NTFY_PARAM` 0x39) plus `UPDT_TRANSFER_DATA` 0x3E and
//!      `UPDT_NTFY_MESSAGE` 0x3F (`v2/table1/Command.java`).
//!    * peripheral (paired device management), table 2 `PERI_*` commands 0x30..=0x39
//!      plus `PERI_SET_EXTENDED_PARAM` 0x3C and `PERI_NTFY_EXTENDED_PARAM` 0x3D
//!      (`v2/table2/Command.java`). Reads of the paired list are denied too: the
//!      reply carries the addresses of other devices.
//!    * reset settings (FunctionType 0xF9, table 1 SYSTEM inquired type 0x09
//!      `RESET_SETTINGS`, carried by `SYSTEM_SET_STATUS` 0xF4 and the SYSTEM family
//!      0xF0..=0xFD): every SYSTEM command with byte 1 = 0x09 and every
//!      `SYSTEM_SET_STATUS` 0xF4, `SYSTEM_SET_PARAM` 0xF8, `SYSTEM_SET_EXT_PARAM` 0xFC,
//!      on both tables. On table 2 inquired type 0x09 is `LINK_AUTO_SWITCH_FOR_SPEAKER`,
//!      not a reset; the deny is kept there as a conservative extra. Also EQEBB inquired type 0x34 (`CUSTOMIZABLE_SOUND_EFFECT_RESET`)
//!      on commands 0x50..=0x5B.
//!    * factory power off: table 1 `POWER_SET_STATUS` 0x24 and `POWER_SET_PARAM` 0x28 with
//!      inquired type 0x03 (`POWER_OFF`), any value (`FACTORY_POWER_OFF` is 0xff).
//!    * a raw `ALERT_SET_PARAM` 0x98. Alert answers exist only through an
//!      [`AlertToken`] ([`Policy::check_alert_answer`]).
//! 2. Read tier (default). Table 1 and 2: `00 00` protocol info, `02 00` capability
//!    info, `04 01|02|03` device info, `06 00` support function. Table 1, only when
//!    the matching FunctionType is in the [`FunctionTable`]:
//!
//!    | Request | Needs function |
//!    |---|---|
//!    | `22 09` LR battery, `22 0a` cradle battery | 0x29, 0x2a |
//!    | `60 <t>` NC/ASM capability, `66 <t>` NC/ASM param | one that maps to `t` (0x6B gives 0x17) |
//!    | `50 00` EQ capability, `56 00` EQ param | 0x50 |
//!    | `d0 <x>`, `d6 <x>` general setting, x = d1, d2 | function x |
//!    | `12 02` codec | 0x12 |
//!
//!    Read requests are exactly 2 bytes.
//! 3. Setting tier, only after [`Policy::with_setting_changes`]:
//!    * `68 17 <status 00|01> <total 00|01> <mode 00|01> <ambient mode 00|01> <level>`.
//!      Level must be inside min..=max of that ambient mode from [`DeviceLimits`]
//!      (parsed NC/ASM capability) and on its step (`(level - min) % step == 0`, when
//!      step > 0), also for NC mode, except that 0 is allowed in NC
//!      mode (the app sends 0). Limits unknown: denied. Needs a function that maps to
//!      inquired type 0x17.
//!    * `58 00 <preset id> 00` select a preset, id must be in the EQ capability.
//!    * `58 00 a0 <n> <steps>` write bands of CUSTOM, `n` equal to the capability band
//!      count and every step below the capability step count. Observed
//!      (`DEVICE-WF-C710N.md` Round 5): bands sent with id 0x16 while CUSTOM was
//!      active overwrote CUSTOM, and bands sent with id 0xa0 while BASS was active were
//!      ignored (the headset answered a bare `59 00`). Only id 0xa0 is allowed. The
//!      device crate must select CUSTOM first and re-read with `56 00`; the policy
//!      checks single frames only.
//!    * `d8 d2 00 <00|01>` sidetone (00 = ON).
//!    * `98 00 <id> 00` alert answer NEGATIVE, only via [`Policy::check_alert_answer`].
//! 4. Disconnecting tier, only after [`Policy::with_disconnecting_changes`], a gate
//!    separate from the setting gate (neither implies the other). These make the
//!    headset drop the link:
//!    * `d8 d1 00 <00|01>` multipoint (the headset raises alert 0x07 first).
//!    * `98 00 07 01` alert answer POSITIVE, only via [`Policy::check_alert_answer`],
//!      only for message id 0x07.

use thiserror::Error;

use crate::frame::{DataType, ReceivedFrame, encode_frame};
use crate::function::{EQ_TYPE_PRESET, FunctionTable, NCASM_TYPE_DUAL_AMBIENT, Table};
use crate::messages::{
    ALERT_MULTIPOINT_DISCONNECT, AlertToken, EQ_PRESET_CUSTOM, EqCapability, Message,
    NcAsmCapability, Request, command, parse_untokened,
};

/// Why a request is hard denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DenyReason {
    /// Data type other than 0x0C and 0x0E.
    #[error("data type {0:#04x} is never permitted")]
    DataType(u8),
    /// Firmware update command family.
    #[error("firmware update (UPDT) command")]
    FirmwareUpdate,
    /// Reset settings.
    #[error("reset settings")]
    ResetSettings,
    /// Paired device management.
    #[error("peripheral (paired device) command")]
    Peripheral,
    /// Factory power off.
    #[error("power off settings")]
    PowerOff,
    /// The function table was built in another link epoch (the link was reset since).
    #[error("function table is from an older link epoch")]
    StaleFunctionTable,
    /// The device limits were taken in another link epoch.
    #[error("device limits are from an older link epoch")]
    StaleLimits,
    /// Raw alert answer.
    #[error("alert answers need an AlertToken")]
    AlertNeedsToken,
}

/// The two opt-in gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// [`Policy::with_setting_changes`].
    Setting,
    /// [`Policy::with_disconnecting_changes`].
    Disconnecting,
}

/// A request was refused by the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PolicyError {
    #[error("hard denied: command {command:#04x}: {reason}")]
    HardDenied { command: u8, reason: DenyReason },
    #[error("not allowlisted: command {command:#04x} on {table:?}")]
    NotAllowlisted { command: u8, table: Table },
    #[error("{tier:?} tier is not enabled")]
    TierNotEnabled { tier: Tier },
    #[error("command {command:#04x}: {reason}")]
    NotSupported { command: u8, reason: &'static str },
    #[error("parameters rejected for command {command:#04x}: {reason}")]
    ParamsNotAllowed { command: u8, reason: &'static str },
    #[error("empty payload")]
    EmptyPayload,
}

/// Limits taken from the parsed capabilities. Empty by default; setters that need a
/// limit are denied until it is known.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceLimits {
    ncasm: Option<NcAsmCapability>,
    eq: Option<EqCapability>,
    epoch: Option<u64>,
}

impl DeviceLimits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take a capability from a received frame: NC/ASM (`61 17 ...`) or EQ (`51 00 ...`)
    /// on `DATA_MDR`. This is the only way to set a limit. Returns true when the frame
    /// was a capability. A capability parsed from raw bytes with [`crate::parse`] cannot
    /// be given to `DeviceLimits`.
    ///
    /// ```compile_fail
    /// let _ = mdr::DeviceLimits { ncasm: None, eq: None, epoch: None };
    /// ```
    pub fn absorb(&mut self, frame: &ReceivedFrame) -> bool {
        if frame.data_type() != DataType::DataMdr {
            return false;
        }
        let msg = parse_untokened(frame).message;
        if !matches!(msg, Message::NcAsmCapability(_) | Message::EqCapability(_)) {
            return false;
        }
        if self.epoch.is_some_and(|e| e != frame.epoch()) {
            self.ncasm = None;
            self.eq = None;
        }
        self.epoch = Some(frame.epoch());
        match msg {
            Message::NcAsmCapability(c) => self.ncasm = Some(c),
            Message::EqCapability(c) => self.eq = Some(c),
            _ => {}
        }
        true
    }

    /// Link epoch of the capability frames, `None` while empty. A capability from another
    /// epoch clears the old limits first.
    pub fn epoch(&self) -> Option<u64> {
        self.epoch
    }

    /// `(min, max, step)` of an ambient mode byte, `None` when unknown.
    fn ncasm_range(&self, ambient_mode: u8) -> Option<(u8, u8, u8)> {
        self.ncasm
            .as_ref()?
            .ranges()
            .iter()
            .find(|r| r.ambient_mode().to_u8() == ambient_mode)
            .map(|r| (r.min(), r.max(), r.step()))
    }
}

/// A request that passed the policy. Not `Clone`, and only [`crate::link::Link::send`]
/// can encode it.
///
/// ```compile_fail
/// let _ = mdr::CheckedRequest {
///     data_type: mdr::DataType::DataMdr,
///     payload: vec![0x00, 0x00],
///     alert_epoch: None,
/// };
/// ```
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<mdr::CheckedRequest>();
/// ```
///
/// ```compile_fail
/// fn f(c: &mdr::CheckedRequest) { let _ = c.encode(0); }
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct CheckedRequest {
    data_type: DataType,
    payload: Vec<u8>,
    alert_epoch: Option<u64>,
    checked_epoch: u64,
}

impl CheckedRequest {
    /// The full wire frame (`3E ... 3C`) for sequence bit `seq` (only the low bit is used).
    pub(crate) fn encode(&self, seq: u8) -> Vec<u8> {
        encode_frame(self.data_type, seq & 1, &self.payload)
    }

    /// The checked payload (command byte first).
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn data_type(&self) -> DataType {
        self.data_type
    }

    /// The link epoch of the token this alert answer was made from, `None` otherwise.
    pub(crate) fn alert_epoch(&self) -> Option<u64> {
        self.alert_epoch
    }

    /// The link epoch this request was checked against. [`crate::link::Link::send`]
    /// refuses it when the link has been reset since (for an alert answer this is the
    /// epoch of the token).
    pub(crate) fn checked_epoch(&self) -> u64 {
        self.checked_epoch
    }
}

/// The allowlist. Read tier by default, gates opt in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Policy {
    setting: bool,
    disconnecting: bool,
}

fn denied(command: u8, reason: DenyReason) -> PolicyError {
    PolicyError::HardDenied { command, reason }
}

fn needs_function(ft: &FunctionTable, id: u8, command: u8) -> Result<(), PolicyError> {
    if ft.contains(Table::One, id) {
        Ok(())
    } else {
        Err(PolicyError::NotSupported {
            command,
            reason: "function not in the support function table",
        })
    }
}

fn params(command: u8, reason: &'static str) -> PolicyError {
    PolicyError::ParamsNotAllowed { command, reason }
}

/// Layer 1. Runs before any tier.
fn hard_deny(table: Table, p: &[u8]) -> Result<(), PolicyError> {
    let cmd = p[0];
    match (table, cmd) {
        (Table::One, 0x30..=0x39 | 0x3E | 0x3F) => {
            return Err(denied(cmd, DenyReason::FirmwareUpdate));
        }
        (Table::Two, 0x30..=0x39 | 0x3C | 0x3D) => {
            return Err(denied(cmd, DenyReason::Peripheral));
        }
        _ => {}
    }
    // SYSTEM family, both tables: any SET status/param/ext param, or inquired type 0x09.
    if matches!(cmd, 0xF4 | 0xF8 | 0xFC) || (matches!(cmd, 0xF0..=0xFD) && p.get(1) == Some(&0x09))
    {
        return Err(denied(cmd, DenyReason::ResetSettings));
    }
    if table == Table::One && matches!(cmd, 0x50..=0x5B) && p.get(1) == Some(&0x34) {
        return Err(denied(cmd, DenyReason::ResetSettings));
    }
    if matches!(cmd, 0x24 | 0x28) && p.get(1) == Some(&0x03) {
        return Err(denied(cmd, DenyReason::PowerOff));
    }
    if table == Table::One && cmd == command::ALERT_SET_PARAM {
        return Err(denied(cmd, DenyReason::AlertNeedsToken));
    }
    Ok(())
}

impl Policy {
    /// Read tier only.
    pub fn new() -> Self {
        Self::default()
    }

    /// Also allow the setting tier (NC/ASM, EQ, sidetone, alert NEGATIVE).
    pub fn with_setting_changes(mut self) -> Self {
        self.setting = true;
        self
    }

    /// Also allow the disconnecting tier (multipoint, alert POSITIVE). Separate gate.
    pub fn with_disconnecting_changes(mut self) -> Self {
        self.disconnecting = true;
        self
    }

    fn require(&self, tier: Tier) -> Result<(), PolicyError> {
        let open = match tier {
            Tier::Setting => self.setting,
            Tier::Disconnecting => self.disconnecting,
        };
        if open {
            Ok(())
        } else {
            Err(PolicyError::TierNotEnabled { tier })
        }
    }

    /// Check a request and return the only thing that can be encoded. `link_epoch` is
    /// [`crate::link::Link::epoch`] of the link the request will be sent on. A request
    /// that consults the function table or the limits is denied when they were built in
    /// another epoch (after [`crate::link::Link::reset`] the old table and limits are
    /// stale until re-absorbed). Requests that consult neither, the connect reads, are
    /// not affected, so the handshake can run again.
    pub fn check(
        &self,
        link_epoch: u64,
        functions: &FunctionTable,
        limits: &DeviceLimits,
        request: &Request,
    ) -> Result<CheckedRequest, PolicyError> {
        let p = request.payload.as_slice();
        let Some(table) = Table::from_data_type(request.data_type) else {
            return Err(denied(
                p.first().copied().unwrap_or(0),
                DenyReason::DataType(request.data_type.to_u8()),
            ));
        };
        if p.is_empty() {
            return Err(PolicyError::EmptyPayload);
        }
        hard_deny(table, p)?;
        self.evaluate(link_epoch, table, functions, limits, p)?;
        Ok(CheckedRequest {
            data_type: request.data_type,
            payload: request.payload.clone(),
            alert_epoch: None,
            checked_epoch: link_epoch,
        })
    }

    fn evaluate(
        &self,
        epoch: u64,
        table: Table,
        ft: &FunctionTable,
        limits: &DeviceLimits,
        p: &[u8],
    ) -> Result<(), PolicyError> {
        let cmd = p[0];
        // Read tier, both tables.
        match p {
            [0x00, 0x00] | [0x02, 0x00] | [0x04, 0x01..=0x03] | [0x06, 0x00] => return Ok(()),
            _ => {}
        }
        if table == Table::Two {
            return Err(PolicyError::NotAllowlisted {
                command: cmd,
                table,
            });
        }
        if ft.epoch().is_some_and(|e| e != epoch) {
            return Err(denied(cmd, DenyReason::StaleFunctionTable));
        }
        let stale_limits = limits.epoch().is_some_and(|e| e != epoch);
        // Staleness: any arm below that reads `limits` must check `stale_limits` first; arms
        // that read only `ft` are covered by the function table epoch check above. The
        // read-tier arms read `ft` only. Link::send also refuses a request checked against
        // another epoch.
        match *p {
            // Read tier, table 1.
            [0x22, 0x09] => needs_function(ft, 0x29, cmd),
            [0x22, 0x0A] => needs_function(ft, 0x2A, cmd),
            [0x60 | 0x66, t] => {
                if ft.has_ncasm_type(t) {
                    Ok(())
                } else {
                    Err(PolicyError::NotSupported {
                        command: cmd,
                        reason: "inquired type not mapped from a present NC/ASM function",
                    })
                }
            }
            [0x50 | 0x56, EQ_TYPE_PRESET] => {
                if ft.has_eq_type(EQ_TYPE_PRESET) {
                    Ok(())
                } else {
                    Err(PolicyError::NotSupported {
                        command: cmd,
                        reason: "PRESET_EQ function not present",
                    })
                }
            }
            [0xD0 | 0xD6, x @ (0xD1 | 0xD2)] => needs_function(ft, x, cmd),
            [0x12, 0x02] => needs_function(ft, 0x12, cmd),
            // Setting tier.
            [0x68, t, status, total, mode, amb, level] => {
                self.require(Tier::Setting)?;
                if stale_limits {
                    return Err(denied(cmd, DenyReason::StaleLimits));
                }
                if t != NCASM_TYPE_DUAL_AMBIENT || !ft.has_ncasm_type(t) {
                    return Err(PolicyError::NotSupported {
                        command: cmd,
                        reason: "only inquired type 0x17 from a present function is settable",
                    });
                }
                if status > 1 || total > 1 || mode > 1 || amb > 1 {
                    return Err(params(
                        cmd,
                        "status, total, mode and ambient mode are 0 or 1",
                    ));
                }
                let Some((min, max, step)) = limits.ncasm_range(amb) else {
                    return Err(params(cmd, "NC/ASM limits unknown"));
                };
                let on_step = step == 0 || (level >= min && (level - min) % step == 0);
                if (mode == 0 && level == 0) || ((min..=max).contains(&level) && on_step) {
                    Ok(())
                } else {
                    Err(params(
                        cmd,
                        "level outside the capability range or off its step",
                    ))
                }
            }
            [0x58, EQ_TYPE_PRESET, id, 0x00] => {
                self.require(Tier::Setting)?;
                if stale_limits {
                    return Err(denied(cmd, DenyReason::StaleLimits));
                }
                if !ft.has_eq_type(EQ_TYPE_PRESET) {
                    return Err(PolicyError::NotSupported {
                        command: cmd,
                        reason: "PRESET_EQ function not present",
                    });
                }
                match &limits.eq {
                    Some(eq) if eq.presets().iter().any(|p| p.id() == id) => Ok(()),
                    Some(_) => Err(params(cmd, "preset id not in the capability")),
                    None => Err(params(cmd, "EQ limits unknown")),
                }
            }
            [0x58, EQ_TYPE_PRESET, id, n, ..] => {
                self.require(Tier::Setting)?;
                if stale_limits {
                    return Err(denied(cmd, DenyReason::StaleLimits));
                }
                if !ft.has_eq_type(EQ_TYPE_PRESET) {
                    return Err(PolicyError::NotSupported {
                        command: cmd,
                        reason: "PRESET_EQ function not present",
                    });
                }
                let Some(eq) = &limits.eq else {
                    return Err(params(cmd, "EQ limits unknown"));
                };
                if id != EQ_PRESET_CUSTOM || !eq.presets().iter().any(|p| p.id() == id) {
                    return Err(params(cmd, "bands may only be written to CUSTOM (0xa0)"));
                }
                let steps = &p[4..];
                if n != eq.band_count() || steps.len() != usize::from(n) {
                    return Err(params(cmd, "band count differs from the capability"));
                }
                if steps.iter().any(|&s| s >= eq.step_count()) {
                    return Err(params(cmd, "band step not below the capability step count"));
                }
                Ok(())
            }
            [0xD8, 0xD2, 0x00, 0x00 | 0x01] => {
                self.require(Tier::Setting)?;
                needs_function(ft, 0xD2, cmd)
            }
            // Disconnecting tier.
            [0xD8, 0xD1, 0x00, 0x00 | 0x01] => {
                self.require(Tier::Disconnecting)?;
                needs_function(ft, 0xD1, cmd)
            }
            _ => Err(PolicyError::NotAllowlisted {
                command: cmd,
                table,
            }),
        }
    }

    /// Answer an alert. `positive` needs the disconnecting gate and message id 0x07,
    /// negative needs the setting gate. The token is consumed.
    pub fn check_alert_answer(
        &self,
        token: AlertToken,
        positive: bool,
    ) -> Result<CheckedRequest, PolicyError> {
        let cmd = command::ALERT_SET_PARAM;
        let id = token.message_id();
        if positive {
            self.require(Tier::Disconnecting)?;
            if id != ALERT_MULTIPOINT_DISCONNECT {
                return Err(params(
                    cmd,
                    "only message 0x07 (multipoint disconnect) may be answered positive",
                ));
            }
        } else {
            self.require(Tier::Setting)?;
        }
        Ok(CheckedRequest {
            data_type: DataType::DataMdr,
            payload: vec![cmd, 0x00, id, u8::from(positive)],
            alert_epoch: Some(token.epoch()),
            checked_epoch: token.epoch(),
        })
    }
}
