//! Safety allowlist and the only way to build bytes for the wire.
//!
//! The transport must build every outgoing frame with [`Policy::check_indexed`].
//! It resolves the wire feature index through the [`FeatureTable`], takes the
//! feature ID and type from that same table row, runs the rules below, and
//! returns a [`CheckedRequest`]. `CheckedRequest` has private fields, no public
//! constructor and is the only type with an `encode()` method, so the bytes that
//! are sent are exactly the bytes that were checked.
//!
//! Index 0 is Root and nothing else. Any other index must be in the table,
//! otherwise the request is refused ([`PolicyError::UnknownFeatureIndex`]). The
//! transport must invalidate its [`FeatureTable`] on a HID++ 1.0 `0x41` device
//! connection notification (the device may have reset or been swapped, so
//! cached indexes can point to different features).
//!
//! Layers, evaluated in this order:
//!
//! 1. Hard deny. Cannot be overridden by any tier or builder:
//!    * feature type with bit 0x40 or 0x20 set (hidden / engineering features),
//!    * feature IDs 0x18xx and 0x1Exx,
//!    * DFU features 0x00C0..=0x00C3 and 0x00D0 (firmware, never touched),
//!    * every 0x8100 (onboard profiles) function other than 0..=8. That excludes
//!      macros and everything from 9 up. SetCurrentProfile (3) is not hard
//!      denied but belongs to the profile select tier only.
//!      Function 1 (SetOnboardMode) is not hard denied but belongs to the
//!      volatile tier only. Functions 6, 7, 8 (BeginWrite, WriteData, EndWrite)
//!      are not hard denied but belong to the onboard write tier only.
//!    * BeginWrite sectors: ROM sectors 0x0101..=0x0105 and every sector outside
//!      [`WRITE_SECTOR_ALLOWLIST`] (see [`DenyReason`]).
//!    * SetCurrentProfile sectors outside 0x0001..=0x0005 (ROM included).
//! 2. Read tier (default).
//! 3. Volatile tier, only after [`Policy::with_volatile_writes`]. These calls
//!    change RAM state of the device (current DPI, current report rate, host or
//!    onboard mode), not flash, and their parameters are bounds checked (see
//!    [`Policy::check_indexed`]).
//! 4. Onboard write tier, only after [`Policy::with_onboard_writes`], a gate
//!    separate from the volatile one (neither implies the other). 0x8100
//!    functions 6, 7, 8 write flash: whole sector only (offset 0, length 255),
//!    only to sectors in [`WRITE_SECTOR_ALLOWLIST`]. The policy checks single
//!    frames; the sequence (6, sixteen times 7, 8, backup and read back) and
//!    what a sector may contain (0x0000 is the profile directory, 3..=5 are
//!    profiles) are the device crate's job.
//! 5. Profile select tier, only after [`Policy::with_profile_select`], a gate
//!    separate from the other two. 0x8100 function 3 (SetCurrentProfile) with the
//!    sector BE16 in `params[0..2]` in 0x0001..=0x0005 and the rest zero.
//! 6. Button spy tier, only after [`Policy::with_button_spy`], a gate separate
//!    from all the others. 0x8110 (MOUSE_BUTTON_SPY) function 1 (start spy) and
//!    function 2 (stop spy), all parameters zero. They switch a RAM-only
//!    diversion of the button events on and off. Functions 3 and 4 (the latter is
//!    the remap table) are in no tier and are denied in every gate.
//!
//! Anything not in an enabled tier is denied. The ReadData sector argument is
//! restricted to 0x0000..=0x00FF and 0x0101..=0x0105.
//!
//! Software IDs on the wire are 2..=15. 0 is the notification marker and 1 is
//! used by the Linux kernel driver. Nothing is masked: invalid values are
//! errors.
//!
//! Note on 0x8100 functions 2 (IsOnboardMode) and 4 (GetActiveProfile): they are
//! reads by the vendor protocol notes, but they have not been exercised on the
//! real device yet (the probe sent only 0x8100 functions 0 and 5).

use thiserror::Error;

use crate::feature::{FeatureId, FeatureTable};
use crate::frame::{LONG_LEN, MAX_SW_ID, MIN_SW_ID, REPORT_ID_LONG};

/// Why a call is hard denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DenyReason {
    /// Feature type has bit 0x40 or 0x20 set (hidden / engineering).
    #[error("hidden or engineering feature type {0:#04x}")]
    HiddenFeatureType(u8),
    /// Feature ID in a forbidden range (0x18xx, 0x1Exx).
    #[error("forbidden feature id range")]
    ForbiddenFeatureId,
    /// DFU (firmware update) feature.
    #[error("DFU (firmware) feature")]
    Dfu,
    /// 0x8100 function other than 0, 1 (SetOnboardMode, volatile tier), the
    /// read functions 2, 4, 5 and the write functions 6, 7, 8 (write tier).
    #[error("onboard profiles function is never permitted (SetActiveProfile, macros, others)")]
    OnboardWriteOrControl,
    /// SetCurrentProfile aimed at a ROM profile sector 0x0101..=0x0105.
    #[error("SetCurrentProfile to ROM sector {0:#06x} is never permitted")]
    RomSectorSelect(u16),
    /// SetCurrentProfile aimed at a sector outside 0x0001..=0x0005.
    #[error("SetCurrentProfile to sector {0:#06x} is not permitted")]
    SectorNotSelectable(u16),
    /// BeginWrite aimed at a ROM profile sector 0x0101..=0x0105.
    #[error("BeginWrite to ROM sector {0:#06x} is never permitted")]
    RomSectorWrite(u16),
    /// BeginWrite aimed at a sector outside [`WRITE_SECTOR_ALLOWLIST`].
    #[error("BeginWrite to sector {0:#06x} is not in the write allowlist")]
    SectorNotWritable(u16),
}

/// A request was refused by the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PolicyError {
    /// Hard denied; no tier or builder can enable it.
    #[error("hard denied: feature {feature_id} function {function}: {reason}")]
    HardDenied {
        feature_id: FeatureId,
        function: u8,
        reason: DenyReason,
    },
    /// Not part of any enabled tier.
    #[error("not allowlisted: feature {feature_id} function {function}")]
    NotAllowlisted { feature_id: FeatureId, function: u8 },
    /// ReadData sector outside 0x0000..=0x00FF and 0x0101..=0x0105.
    #[error("ReadData sector {sector:#06x} is not allowed")]
    SectorNotAllowed { sector: u16 },
    /// The wire feature index is not in the feature table (index 0 is Root).
    #[error("feature index {feature_index} is not in the feature table")]
    UnknownFeatureIndex { feature_index: u8 },
    /// Function number does not fit in a nibble.
    #[error("function {0} out of range 0..=15")]
    InvalidFunction(u8),
    /// Software ID outside 2..=15.
    #[error("software id {0} out of range 2..=15")]
    InvalidSwId(u8),
    /// A volatile setter was called with parameters outside its bounds.
    #[error("parameters rejected for feature {feature_id} function {function}: {reason}")]
    ParamsNotAllowed {
        feature_id: FeatureId,
        function: u8,
        reason: &'static str,
    },
}

/// Read tier: (feature ID, permitted functions).
const READ_TIER: &[(FeatureId, &[u8])] = &[
    (FeatureId::ROOT, &[0]),
    (FeatureId::FEATURE_SET, &[0, 1]),
    (FeatureId::ADJUSTABLE_DPI, &[0, 1, 2]),
    (FeatureId::REPORT_RATE, &[0, 1]),
    (FeatureId::UNIFIED_BATTERY, &[1]),
    // 2 IsOnboardMode and 4 GetActiveProfile: reads, not yet exercised on the
    // device. 5 is ReadData, further restricted by the sector check.
    (FeatureId::ONBOARD_PROFILES, &[0, 2, 4, 5]),
];

/// Volatile tier: RAM-only setters. SetSensorDPI (0x2201 fn 3), SetReportRate
/// (0x8060 fn 2), SetOnboardMode (0x8100 fn 1, host/onboard switch; selects who
/// runs the profile logic, writes no flash). Nothing else, ever.
const VOLATILE_TIER: &[(FeatureId, &[u8])] = &[
    (FeatureId::ADJUSTABLE_DPI, &[3]),
    (FeatureId::REPORT_RATE, &[2]),
    (FeatureId::ONBOARD_PROFILES, &[1]),
];

/// Onboard write tier: flash writes of whole user profile sectors, 0x8100
/// BeginWrite (6), WriteData (7), EndWrite (8). Nothing else, ever.
const ONBOARD_WRITE_TIER: &[(FeatureId, &[u8])] = &[(FeatureId::ONBOARD_PROFILES, &[6, 7, 8])];

/// Profile select tier: 0x8100 SetCurrentProfile (3). Nothing else.
const PROFILE_SELECT_TIER: &[(FeatureId, &[u8])] = &[(FeatureId::ONBOARD_PROFILES, &[3])];

/// Button spy tier: 0x8110 start (1) and stop (2). Nothing else, ever.
const BUTTON_SPY_TIER: &[(FeatureId, &[u8])] = &[(FeatureId::MOUSE_BUTTON_SPY, &[1, 2])];

/// 0x8100 SetCurrentProfile function number (profile select tier).
pub const ONBOARD_FN_SET_PROFILE: u8 = 3;
/// Sectors SetCurrentProfile may name (user sectors).
pub const SELECT_SECTORS: std::ops::RangeInclusive<u16> = 0x0001..=0x0005;
/// 0x8100 SetOnboardMode function number (volatile tier).
pub const ONBOARD_FN_SET_MODE: u8 = 1;
/// 0x8100 ReadData function number.
pub const ONBOARD_FN_READ_DATA: u8 = 5;
/// 0x8100 BeginWrite function number (onboard write tier).
pub const ONBOARD_FN_BEGIN_WRITE: u8 = 6;
/// 0x8100 WriteData function number (onboard write tier).
pub const ONBOARD_FN_WRITE_DATA: u8 = 7;
/// 0x8100 EndWrite function number (onboard write tier).
pub const ONBOARD_FN_END_WRITE: u8 = 8;
/// Sectors BeginWrite may target: the directory 0x0000 and the user profile
/// sectors 0x0003..=0x0005. 0x0001, 0x0002 (the user's profiles) and the ROM
/// sectors are excluded. The policy sees single frames, so it cannot tell what
/// goes into a sector: the device crate's write functions keep profile images
/// out of 0x0000 and directory images out of 0x0003..=0x0005.
pub const WRITE_SECTOR_ALLOWLIST: &[u16] = &[0x0000, 0x0003, 0x0004, 0x0005];
/// The only BeginWrite offset.
pub const WRITE_OFFSET: u16 = 0;
/// The only BeginWrite length (whole sector).
pub const WRITE_LENGTH: u16 = 255;

fn tier_allows(tier: &[(FeatureId, &[u8])], id: FeatureId, function: u8) -> bool {
    tier.iter()
        .any(|(fid, fns)| *fid == id && fns.contains(&function))
}

fn hard_deny_reason(id: FeatureId, feature_type: u8, function: u8) -> Option<DenyReason> {
    if feature_type & 0x60 != 0 {
        return Some(DenyReason::HiddenFeatureType(feature_type));
    }
    let hi = id.0 >> 8;
    if hi == 0x18 || hi == 0x1E {
        return Some(DenyReason::ForbiddenFeatureId);
    }
    if (0x00C0..=0x00C3).contains(&id.0) || id.0 == 0x00D0 {
        return Some(DenyReason::Dfu);
    }
    if id == FeatureId::ONBOARD_PROFILES && !matches!(function, 0..=8) {
        return Some(DenyReason::OnboardWriteOrControl);
    }
    None
}

/// `true` for sectors that may be read: 0x0000..=0x00FF (directory and user
/// sectors) and 0x0101..=0x0105 (ROM profiles).
pub fn read_sector_allowed(sector: u16) -> bool {
    sector <= 0x00FF || (0x0101..=0x0105).contains(&sector)
}

/// SetCurrentProfile sector rule: user sectors 1..=5 only; ROM and the rest are
/// hard denied with distinct reasons.
fn check_select_sector(sector: u16) -> Result<(), DenyReason> {
    if (0x0101..=0x0105).contains(&sector) {
        return Err(DenyReason::RomSectorSelect(sector));
    }
    if !SELECT_SECTORS.contains(&sector) {
        return Err(DenyReason::SectorNotSelectable(sector));
    }
    Ok(())
}

/// BeginWrite sector rule: ROM sectors and anything outside the allowlist are
/// hard denied with distinct reasons.
fn check_write_sector(sector: u16) -> Result<(), DenyReason> {
    if (0x0101..=0x0105).contains(&sector) {
        return Err(DenyReason::RomSectorWrite(sector));
    }
    if !WRITE_SECTOR_ALLOWLIST.contains(&sector) {
        return Err(DenyReason::SectorNotWritable(sector));
    }
    Ok(())
}

/// Parameter bounds of the volatile setters and the write functions. `Ok` for
/// every other call.
fn param_bounds(id: FeatureId, function: u8, params: &[u8; 16]) -> Result<(), &'static str> {
    if id == FeatureId::MOUSE_BUTTON_SPY {
        // Start and stop take no parameters.
        if params.iter().any(|&b| b != 0) {
            return Err("button spy takes no parameters");
        }
    } else if id == FeatureId::ONBOARD_PROFILES && function == ONBOARD_FN_SET_PROFILE {
        if params[2..].iter().any(|&b| b != 0) {
            return Err("unused parameter bytes must be zero");
        }
    } else if id == FeatureId::ONBOARD_PROFILES && function == ONBOARD_FN_BEGIN_WRITE {
        if u16::from_be_bytes([params[2], params[3]]) != WRITE_OFFSET {
            return Err("write offset must be 0");
        }
        if u16::from_be_bytes([params[4], params[5]]) != WRITE_LENGTH {
            return Err("write length must be 255");
        }
        if params[6..].iter().any(|&b| b != 0) {
            return Err("unused parameter bytes must be zero");
        }
    } else if id == FeatureId::ONBOARD_PROFILES && function == ONBOARD_FN_END_WRITE {
        if params.iter().any(|&b| b != 0) {
            return Err("EndWrite takes no parameters");
        }
    } else if id == FeatureId::REPORT_RATE && function == 2 {
        if !(1..=8).contains(&params[0]) {
            return Err("report interval must be 1..=8 ms");
        }
        if params[1..].iter().any(|&b| b != 0) {
            return Err("unused parameter bytes must be zero");
        }
    } else if id == FeatureId::ONBOARD_PROFILES && function == ONBOARD_FN_SET_MODE {
        // 0x01 onboard, 0x02 host (wire values); nothing else follows.
        if !matches!(params[0], 0x01 | 0x02) {
            return Err("onboard mode must be 0x01 or 0x02");
        }
        if params[1..].iter().any(|&b| b != 0) {
            return Err("unused parameter bytes must be zero");
        }
    } else if id == FeatureId::ADJUSTABLE_DPI && function == 3 {
        // Sensor 0 only, and the dpiLevel byte (params[3], feature version >= 2)
        // is pinned to 0 (BRIEF [DER]: MyHUB sends 0).
        if params[0] != 0 {
            return Err("sensor must be 0");
        }
        if params[3] != 0 {
            return Err("dpiLevel must be 0");
        }
        if params[1] == 0 && params[2] == 0 {
            return Err("DPI must be non-zero");
        }
        if params[4..].iter().any(|&b| b != 0) {
            return Err("unused parameter bytes must be zero");
        }
    }
    Ok(())
}

/// A request that passed the policy. Its fields are private and it can only be
/// created by [`Policy::check_indexed`]; it is the only type that encodes to
/// wire bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedRequest {
    device_index: u8,
    feature_index: u8,
    function: u8,
    sw_id: u8,
    params: [u8; 16],
}

impl CheckedRequest {
    pub fn device_index(&self) -> u8 {
        self.device_index
    }

    pub fn feature_index(&self) -> u8 {
        self.feature_index
    }

    /// Function number, 0..=15.
    pub fn function(&self) -> u8 {
        self.function
    }

    /// Software ID, 2..=15.
    pub fn sw_id(&self) -> u8 {
        self.sw_id
    }

    pub fn params(&self) -> &[u8; 16] {
        &self.params
    }

    /// Encodes to a 20 byte long report. Values were validated when the
    /// request was checked; nothing is masked here.
    pub fn encode(&self) -> [u8; LONG_LEN] {
        let mut out = [0u8; LONG_LEN];
        out[0] = REPORT_ID_LONG;
        out[1] = self.device_index;
        out[2] = self.feature_index;
        out[3] = (self.function << 4) | self.sw_id;
        out[4..].copy_from_slice(&self.params);
        out
    }
}

/// The safety allowlist. Read-only by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Policy {
    volatile_writes: bool,
    onboard_writes: bool,
    profile_select: bool,
    button_spy: bool,
}

impl Policy {
    /// Read tier only.
    pub const fn new() -> Self {
        Self {
            volatile_writes: false,
            onboard_writes: false,
            profile_select: false,
            button_spy: false,
        }
    }

    /// Additionally enables the button spy tier (0x8110 start and stop). Independent
    /// of every other gate.
    pub const fn with_button_spy(mut self) -> Self {
        self.button_spy = true;
        self
    }

    /// `true` when the button spy tier is enabled.
    pub const fn button_spy_enabled(&self) -> bool {
        self.button_spy
    }

    /// Additionally enables the profile select tier (0x8100 SetCurrentProfile).
    /// Independent of the volatile and onboard write gates.
    pub const fn with_profile_select(mut self) -> Self {
        self.profile_select = true;
        self
    }

    /// `true` when the profile select tier is enabled.
    pub const fn profile_select_enabled(&self) -> bool {
        self.profile_select
    }

    /// Additionally enables the onboard write tier (0x8100 BeginWrite,
    /// WriteData, EndWrite: flash). Independent of the volatile gate. The hard
    /// deny list and the sector, offset and length bounds stay in force.
    pub const fn with_onboard_writes(mut self) -> Self {
        self.onboard_writes = true;
        self
    }

    /// `true` when the onboard write tier is enabled.
    pub const fn onboard_writes_enabled(&self) -> bool {
        self.onboard_writes
    }

    /// Additionally enables the volatile tier (SetSensorDPI, SetReportRate,
    /// SetOnboardMode). The hard deny list stays in force.
    pub const fn with_volatile_writes(mut self) -> Self {
        self.volatile_writes = true;
        self
    }

    /// `true` when the volatile tier is enabled.
    pub const fn volatile_writes_enabled(&self) -> bool {
        self.volatile_writes
    }

    /// Checks one (feature, function) pair. `feature_type` is the type byte
    /// from FeatureSet GetFeatureID (0 for Root).
    pub(crate) fn check(
        &self,
        feature_id: FeatureId,
        feature_type: u8,
        function: u8,
    ) -> Result<(), PolicyError> {
        if let Some(reason) = hard_deny_reason(feature_id, feature_type, function) {
            return Err(PolicyError::HardDenied {
                feature_id,
                function,
                reason,
            });
        }
        if tier_allows(READ_TIER, feature_id, function)
            || (self.volatile_writes && tier_allows(VOLATILE_TIER, feature_id, function))
            || (self.onboard_writes && tier_allows(ONBOARD_WRITE_TIER, feature_id, function))
            || (self.profile_select && tier_allows(PROFILE_SELECT_TIER, feature_id, function))
            || (self.button_spy && tier_allows(BUTTON_SPY_TIER, feature_id, function))
        {
            return Ok(());
        }
        Err(PolicyError::NotAllowlisted {
            feature_id,
            function,
        })
    }

    /// Checks the sector argument of a 0x8100 ReadData request.
    pub fn check_read_data_sector(&self, sector: u16) -> Result<(), PolicyError> {
        if read_sector_allowed(sector) {
            Ok(())
        } else {
            Err(PolicyError::SectorNotAllowed { sector })
        }
    }

    /// [`Policy::check`] plus the argument rules: the ReadData sector bound
    /// (sector is `params[0..2]` big-endian) and the volatile setter bounds.
    pub(crate) fn check_request(
        &self,
        feature_id: FeatureId,
        feature_type: u8,
        function: u8,
        params: &[u8; 16],
    ) -> Result<(), PolicyError> {
        self.check(feature_id, feature_type, function)?;
        if feature_id == FeatureId::ONBOARD_PROFILES && function == ONBOARD_FN_READ_DATA {
            self.check_read_data_sector(u16::from_be_bytes([params[0], params[1]]))?;
        }
        if feature_id == FeatureId::ONBOARD_PROFILES && function == ONBOARD_FN_SET_PROFILE {
            check_select_sector(u16::from_be_bytes([params[0], params[1]])).map_err(|reason| {
                PolicyError::HardDenied {
                    feature_id,
                    function,
                    reason,
                }
            })?;
        }
        if feature_id == FeatureId::ONBOARD_PROFILES && function == ONBOARD_FN_BEGIN_WRITE {
            check_write_sector(u16::from_be_bytes([params[0], params[1]])).map_err(|reason| {
                PolicyError::HardDenied {
                    feature_id,
                    function,
                    reason,
                }
            })?;
        }
        param_bounds(feature_id, function, params).map_err(|reason| PolicyError::ParamsNotAllowed {
            feature_id,
            function,
            reason,
        })
    }

    /// The pre-send gate. Validates `function` (0..=15) and `sw_id` (2..=15),
    /// resolves `feature_index` through `table` (index 0 is Root, any other
    /// index must be in the table, else `UnknownFeatureIndex`), takes the
    /// feature ID and type from that same row, applies the hard deny list, the
    /// enabled tiers, the ReadData sector bound and the volatile parameter
    /// bounds, and returns the only value that can be encoded to wire bytes.
    ///
    /// Volatile parameter bounds (only reachable after
    /// [`Policy::with_volatile_writes`]): 0x8060 SetReportRate needs
    /// `params[0]` in 1..=8 and `params[1..]` zero; 0x8100 SetOnboardMode needs
    /// `params[0]` 0x01 or 0x02 and `params[1..]` zero; 0x2201 SetSensorDPI needs a
    /// non-zero DPI in `params[1..3]` with sensor `params[0]`, dpiLevel
    /// `params[3]` and `params[4..]` all zero.
    ///
    /// Onboard write bounds (only reachable after
    /// [`Policy::with_onboard_writes`]): 0x8100 BeginWrite (fn 6) needs the
    /// sector in `params[0..2]` (big-endian) to be in
    /// [`WRITE_SECTOR_ALLOWLIST`] (ROM sectors and all others are hard denied),
    /// offset `params[2..4]` 0, length `params[4..6]` 255 and `params[6..]`
    /// zero; EndWrite (fn 8) needs all 16 parameter bytes zero; WriteData
    /// (fn 7) carries 16 data bytes and is not inspected.
    ///
    /// Button spy bounds (only reachable after [`Policy::with_button_spy`]): 0x8110
    /// start (fn 1) and stop (fn 2) need all 16 parameter bytes zero.
    ///
    /// Profile select bounds (only reachable after
    /// [`Policy::with_profile_select`]): 0x8100 SetCurrentProfile (fn 3) needs
    /// the sector in `params[0..2]` (big-endian) in 0x0001..=0x0005 (ROM sectors
    /// and all others are hard denied) and `params[2..]` zero.
    pub fn check_indexed(
        &self,
        table: &FeatureTable,
        device_index: u8,
        feature_index: u8,
        function: u8,
        sw_id: u8,
        params: [u8; 16],
    ) -> Result<CheckedRequest, PolicyError> {
        if !(MIN_SW_ID..=MAX_SW_ID).contains(&sw_id) {
            return Err(PolicyError::InvalidSwId(sw_id));
        }
        if function > 0x0F {
            return Err(PolicyError::InvalidFunction(function));
        }
        let (feature_id, feature_type) = if feature_index == 0 {
            (FeatureId::ROOT, 0)
        } else {
            let row = table
                .by_index(feature_index)
                .ok_or(PolicyError::UnknownFeatureIndex { feature_index })?;
            (row.id, row.feature_type)
        };
        self.check_request(feature_id, feature_type, function, &params)?;
        Ok(CheckedRequest {
            device_index,
            feature_index,
            function,
            sw_id,
            params,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent copy of the allowlist for the sweep.
    fn expected(id: u16, f: u8, volatile: bool, onboard: bool, select: bool, spy: bool) -> bool {
        let read = matches!(
            (id, f),
            (0x0000, 0)
                | (0x0001, 0 | 1)
                | (0x2201, 0..=2)
                | (0x8060, 0 | 1)
                | (0x1004, 1)
                | (0x8100, 0 | 2 | 4 | 5)
        );
        let vol = volatile && matches!((id, f), (0x2201, 3) | (0x8060, 2) | (0x8100, 1));
        let write = onboard && matches!((id, f), (0x8100, 6..=8));
        let sel = select && matches!((id, f), (0x8100, 3));
        let spy_ok = spy && matches!((id, f), (0x8110, 1 | 2));
        read || vol || write || sel || spy_ok
    }

    /// Every u16 feature ID x functions 0..=31 x types x both modes.
    #[test]
    fn exhaustive_sweep_of_check() {
        let hidden = [0x20u8, 0x40, 0x60, 0x68, 0x70];
        let plain = [0x00u8, 0x80];
        for combo in 0..16u8 {
            let (volatile, onboard, select, spy) = (
                combo & 1 != 0,
                combo & 2 != 0,
                combo & 4 != 0,
                combo & 8 != 0,
            );
            let mut policy = Policy::new();
            if volatile {
                policy = policy.with_volatile_writes();
            }
            if onboard {
                policy = policy.with_onboard_writes();
            }
            if select {
                policy = policy.with_profile_select();
            }
            if spy {
                policy = policy.with_button_spy();
            }
            for id in 0..=u16::MAX {
                for f in 0..=31u8 {
                    for &t in &hidden {
                        assert!(
                            matches!(
                                policy.check(FeatureId(id), t, f),
                                Err(PolicyError::HardDenied { .. })
                            ),
                            "hidden type {t:#04x} id {id:#06x} fn {f}"
                        );
                    }
                    for &t in &plain {
                        let ok = policy.check(FeatureId(id), t, f).is_ok();
                        assert_eq!(
                            ok,
                            expected(id, f, volatile, onboard, select, spy),
                            "type {t:#04x} id {id:#06x} fn {f} volatile={volatile} onboard={onboard} select={select} spy={spy}"
                        );
                    }
                }
            }
        }
    }
}
