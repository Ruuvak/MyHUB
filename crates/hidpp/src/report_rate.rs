//! Report rate (0x8060) codecs.
//!
//! Function numbers: 0 GetReportRateList, 1 GetReportRate, 2 SetReportRate
//! (volatile, see policy). Intervals are in milliseconds, 1..=8.

use serde::Serialize;

use crate::error::{EncodeError, ParseError};

pub const FN_GET_REPORT_RATE_LIST: u8 = 0;
pub const FN_GET_REPORT_RATE: u8 = 1;
pub const FN_SET_REPORT_RATE: u8 = 2;

/// Supported intervals as reported by GetReportRateList. Bit `n` of the
/// bitmask means an interval of `n + 1` ms. \[OBS\] on the device (0x8B). The
/// vendor notes call the bitmask layout \[INF\]; the fixture is consistent with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReportRateList {
    pub bitmask: u8,
}

impl ReportRateList {
    /// Parses a GetReportRateList reply payload (byte 0).
    pub fn parse(payload: &[u8]) -> Result<Self, ParseError> {
        let Some(&bitmask) = payload.first() else {
            return Err(ParseError::TooShort { need: 1, got: 0 });
        };
        Ok(Self { bitmask })
    }

    /// Supported intervals in ms, ascending.
    pub fn intervals_ms(&self) -> Vec<u8> {
        (0..8u8)
            .filter(|n| self.bitmask & (1 << n) != 0)
            .map(|n| n + 1)
            .collect()
    }

    /// Supported rates in Hz (1000 / interval), fastest first.
    pub fn rates_hz(&self) -> Vec<u16> {
        self.intervals_ms()
            .iter()
            .map(|&ms| interval_to_hz(ms))
            .collect()
    }

    /// `true` when `interval_ms` (1..=8) is supported.
    pub fn supports(&self, interval_ms: u8) -> bool {
        (1..=8).contains(&interval_ms) && self.bitmask & (1 << (interval_ms - 1)) != 0
    }
}

/// Converts an interval in ms to a rate in Hz. 1 ms = 1000 Hz, 8 ms = 125 Hz.
/// `interval_ms` must be non-zero.
pub fn interval_to_hz(interval_ms: u8) -> u16 {
    1000 / u16::from(interval_ms.max(1))
}

/// Parses a GetReportRate reply payload: byte 0 is the interval in ms, valid
/// 1..=8 \[OBS\].
pub fn parse_current_interval_ms(payload: &[u8]) -> Result<u8, ParseError> {
    let Some(&interval) = payload.first() else {
        return Err(ParseError::TooShort { need: 1, got: 0 });
    };
    if !(1..=8).contains(&interval) {
        return Err(ParseError::InvalidValue {
            field: "report interval (ms)",
            value: u32::from(interval),
        });
    }
    Ok(interval)
}

/// Builds the parameters of SetReportRate: `[interval ms]`, validated against
/// the device's list. Pure: nothing is sent.
pub fn set_report_rate_params(
    interval_ms: u8,
    list: &ReportRateList,
) -> Result<[u8; 16], EncodeError> {
    if !list.supports(interval_ms) {
        return Err(EncodeError::RateNotSupported { interval_ms });
    }
    let mut params = [0u8; 16];
    params[0] = interval_ms;
    Ok(params)
}
