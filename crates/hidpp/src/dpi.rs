//! Adjustable DPI (0x2201) codecs.
//!
//! The G502 X exposes 0x2201 version 2 \[OBS\]. Function numbers: 0 GetSensorCount,
//! 1 GetSensorDPIList, 2 GetSensorDPI, 3 SetSensorDPI (volatile, see policy).

use serde::Serialize;

use crate::error::{EncodeError, ParseError};

pub const FN_GET_SENSOR_COUNT: u8 = 0;
pub const FN_GET_SENSOR_DPI_LIST: u8 = 1;
pub const FN_GET_SENSOR_DPI: u8 = 2;
pub const FN_SET_SENSOR_DPI: u8 = 3;

fn need(payload: &[u8], need: usize) -> Result<(), ParseError> {
    if payload.len() < need {
        Err(ParseError::TooShort {
            need,
            got: payload.len(),
        })
    } else {
        Ok(())
    }
}

/// Parses GetSensorCount: byte 0.
pub fn parse_sensor_count(payload: &[u8]) -> Result<u8, ParseError> {
    need(payload, 1)?;
    Ok(payload[0])
}

/// Request parameters for GetSensorDPIList / GetSensorDPI.
pub fn sensor_params(sensor: u8) -> [u8; 16] {
    let mut params = [0u8; 16];
    params[0] = sensor;
    params
}

/// One element of a DPI list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DpiEntry {
    /// A single supported value.
    Discrete(u16),
    /// `min..=max` in increments of `step`.
    Range { min: u16, max: u16, step: u16 },
}

impl DpiEntry {
    fn contains(&self, dpi: u16) -> bool {
        match *self {
            Self::Discrete(v) => v == dpi,
            Self::Range { min, max, step } => {
                dpi >= min && dpi <= max && (dpi - min).is_multiple_of(step)
            }
        }
    }
}

/// Parsed GetSensorDPIList reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DpiList {
    pub sensor: u8,
    pub entries: Vec<DpiEntry>,
}

impl DpiList {
    /// Parses a GetSensorDPIList reply payload: byte 0 is the sensor index,
    /// then BE16 words. A word whose first byte is 0xE0 is a step marker with
    /// step = second byte; the previous word is the range start and the next
    /// word the range end. 0x0000 terminates the list.
    pub fn parse(payload: &[u8]) -> Result<Self, ParseError> {
        need(payload, 3)?;
        let sensor = payload[0];
        let mut entries: Vec<DpiEntry> = Vec::new();
        let (pairs, _) = payload[1..].as_chunks::<2>();
        let mut words = pairs.iter();
        while let Some(word) = words.next() {
            let (hi, lo) = (word[0], word[1]);
            if hi == 0 && lo == 0 {
                break;
            }
            if hi == 0xE0 {
                let step = u16::from(lo);
                if step == 0 {
                    return Err(ParseError::MalformedDpiList("step marker with step 0"));
                }
                let Some(DpiEntry::Discrete(min)) = entries.pop() else {
                    return Err(ParseError::MalformedDpiList(
                        "step marker without a preceding start value",
                    ));
                };
                let Some(end) = words.next() else {
                    return Err(ParseError::MalformedDpiList(
                        "step marker without an end value",
                    ));
                };
                if end[0] == 0xE0 || (end[0] == 0 && end[1] == 0) {
                    return Err(ParseError::MalformedDpiList("invalid range end value"));
                }
                let max = u16::from_be_bytes([end[0], end[1]]);
                if max < min {
                    return Err(ParseError::MalformedDpiList("range end below start"));
                }
                entries.push(DpiEntry::Range { min, max, step });
            } else {
                entries.push(DpiEntry::Discrete(u16::from_be_bytes([hi, lo])));
            }
        }
        Ok(Self { sensor, entries })
    }

    /// `true` when `dpi` is in the list: equal to a discrete value, or inside a
    /// range and on its step.
    pub fn contains(&self, dpi: u16) -> bool {
        self.entries.iter().any(|e| e.contains(dpi))
    }
}

/// Parsed GetSensorDPI reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SensorDpi {
    pub sensor: u8,
    pub current: u16,
    pub default: u16,
}

/// Parses a GetSensorDPI reply payload: sensor, current BE16, default BE16.
pub fn parse_sensor_dpi(payload: &[u8]) -> Result<SensorDpi, ParseError> {
    need(payload, 5)?;
    Ok(SensorDpi {
        sensor: payload[0],
        current: u16::from_be_bytes([payload[1], payload[2]]),
        default: u16::from_be_bytes([payload[3], payload[4]]),
    })
}

/// Builds the parameters of SetSensorDPI: `[sensor, dpi BE16, level]`. The DPI
/// is validated against `list` (range and step). Pure: nothing is sent.
///
/// The level byte exists only when `feature_version >= 2`, and for those
/// versions an explicit `level` is required (`None` is an error), because the
/// meaning of level 0 is \[INF\] (unverified) and must not be chosen implicitly.
/// For versions below 2, `level` must be `None`.
pub fn set_sensor_dpi_params(
    sensor: u8,
    dpi: u16,
    level: Option<u8>,
    feature_version: u8,
    list: &DpiList,
) -> Result<[u8; 16], EncodeError> {
    if sensor != list.sensor {
        return Err(EncodeError::SensorMismatch {
            sensor,
            list_sensor: list.sensor,
        });
    }
    if !list.contains(dpi) {
        return Err(EncodeError::DpiNotSupported { dpi });
    }
    let mut params = [0u8; 16];
    params[0] = sensor;
    params[1..3].copy_from_slice(&dpi.to_be_bytes());
    match (level, feature_version >= 2) {
        (Some(level), true) => params[3] = level,
        (None, false) => {}
        (Some(_), false) => {
            return Err(EncodeError::DpiLevelNeedsV2 {
                version: feature_version,
            });
        }
        (None, true) => {
            return Err(EncodeError::DpiLevelRequired {
                version: feature_version,
            });
        }
    }
    Ok(params)
}
