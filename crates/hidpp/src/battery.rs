//! Unified battery (0x1004) GetStatus parser.

use serde::Serialize;

use crate::error::ParseError;

/// 0x1004 function 1: GetStatus.
pub const FN_GET_STATUS: u8 = 1;

/// Decoded 0x1004 v3 GetStatus reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BatteryStatus {
    /// State of charge in percent, byte 0 \[OBS\] (fixture 0x37 = 55 %, matches
    /// the vendor agent).
    pub percent: u8,
    /// Byte 1, battery level flags. Meaning is \[INF\] (public 0x1004 spec),
    /// fixture 0x04.
    pub level_flags: u8,
    /// Byte 2, charging state. Meaning is \[INF\], fixture 0x00.
    pub charging_state: u8,
    /// Byte 3, external power status. Meaning is \[INF\], fixture 0x00.
    pub external_power: u8,
}

/// Parses a GetStatus reply payload.
pub fn parse_status(payload: &[u8]) -> Result<BatteryStatus, ParseError> {
    if payload.len() < 4 {
        return Err(ParseError::TooShort {
            need: 4,
            got: payload.len(),
        });
    }
    Ok(BatteryStatus {
        percent: payload[0],
        level_flags: payload[1],
        charging_state: payload[2],
        external_power: payload[3],
    })
}

/// Name of the level byte (byte 1) when it is exactly one of the public 0x1004
/// level flags \[INF\]: 8 full, 4 good, 2 low, 1 critical. `None` otherwise.
pub fn level_name(level_flags: u8) -> Option<&'static str> {
    match level_flags {
        8 => Some("full"),
        4 => Some("good"),
        2 => Some("low"),
        1 => Some("critical"),
        _ => None,
    }
}

/// Name of the charging state byte (byte 2), Solaar's names \[INF\]:
/// `unknown(n)` for any other value.
pub fn charging_state_name(charging_state: u8) -> String {
    match charging_state {
        0 => "discharging".into(),
        1 => "recharging".into(),
        2 => "almost_full".into(),
        3 => "full".into(),
        4 => "slow_recharge".into(),
        5 => "invalid_battery".into(),
        6 => "thermal_error".into(),
        n => format!("unknown({n})"),
    }
}

impl BatteryStatus {
    /// See [`level_name`].
    pub fn level_name(&self) -> Option<&'static str> {
        level_name(self.level_flags)
    }

    /// See [`charging_state_name`].
    pub fn charging_state_name(&self) -> String {
        charging_state_name(self.charging_state)
    }
}
