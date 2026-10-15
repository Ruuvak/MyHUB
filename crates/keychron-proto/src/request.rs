//! Typed requests. Callers never pass raw bytes: a [`Request`] is data, and only
//! [`crate::policy::Policy::check`] turns it into a [`crate::policy::CheckedRequest`],
//! the one type with `encode()`.

use serde::Serialize;

use crate::frame::{FRAME_LEN, Frame};
use crate::policy::{Policy, PolicyError, Tier};

/// Global effect ids 0..=24 [OBS definition, lighting.md section 4].
pub const EFFECT_MAX: u8 = 24;
/// Actuation in 0.1 mm: 0.2 to 3.8 mm [INF, he.md 4.1].
pub const ACTUATION_RANGE: (u8, u8) = (2, 38);
/// Rapid trigger press and release sensitivity in 0.1 mm [INF, he.md 4.1]. he.md 4.1 also
/// says "sensitivity <= max_distance - actuation" [INF]; that is not enforced here. The
/// device layer re-reads the profile buffer after a write to see what the keyboard kept.
pub const SENSITIVITY_RANGE: (u8, u8) = (1, 38);
/// HE profile buffer chunk, bytes 6..32 of the reply [OBS he.md 3.1].
pub const HE_CHUNK_MAX: u8 = 26;
/// Keymap and macro chunk, bytes 4..32 of the reply.
pub const VIA_CHUNK_MAX: u8 = 28;
/// Per-key colour triples per frame (launcher chunks of 9) [INF].
pub const LED_COLORS_MAX: u8 = 9;
/// Region ids per frame (3 + 29 = 32) [INF].
pub const MIX_REGIONS_MAX: u8 = 29;
/// Mix slots per frame (3 + 3 * 8) [INF].
pub const MIX_SLOTS_MAX: u8 = 3;

/// The four VIA channel 3 lighting values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LightId {
    Brightness,
    Effect,
    Speed,
    Color,
}

impl LightId {
    pub(crate) fn value_id(self) -> u8 {
        match self {
            LightId::Brightness => 1,
            LightId::Effect => 2,
            LightId::Speed => 3,
            LightId::Color => 4,
        }
    }

    pub(crate) fn from_value_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(LightId::Brightness),
            2 => Some(LightId::Effect),
            3 => Some(LightId::Speed),
            4 => Some(LightId::Color),
            _ => None,
        }
    }
}

/// A lighting value, used for reads (decoded) and for [`Request::SetLight`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LightValue {
    Brightness(u8),
    Effect(u8),
    Speed(u8),
    Color { hue: u8, saturation: u8 },
}

impl LightValue {
    pub fn id(self) -> LightId {
        match self {
            LightValue::Brightness(_) => LightId::Brightness,
            LightValue::Effect(_) => LightId::Effect,
            LightValue::Speed(_) => LightId::Speed,
            LightValue::Color { .. } => LightId::Color,
        }
    }
}

/// Work mode of SET_TRAVAL in v1. Global (0) is not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TravelMode {
    Static,
    RapidTrigger,
}

impl TravelMode {
    fn wire(self) -> u8 {
        match self {
            TravelMode::Static => 1,
            TravelMode::RapidTrigger => 2,
        }
    }
}

/// Every request the crate can express. The allowlist is exactly these variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    // Read tier, VIA.
    /// `01`.
    GetViaProtocol,
    /// `02 04`.
    GetViaJsonVersion,
    /// `08 03 id`.
    GetLight { id: LightId },
    /// `11`.
    GetLayerCount,
    /// `12 off_hi off_lo len`, offset in bytes, big-endian.
    GetKeymapBuffer { offset: u16, len: u8 },
    /// `0C`.
    GetMacroCount,
    /// `0D`.
    GetMacroBufferSize,
    /// `0E off_hi off_lo len`.
    GetMacroBuffer { offset: u16, len: u8 },
    // Read tier, Keychron system.
    /// `A0`.
    GetKcVersion,
    /// `A1`.
    GetFirmware,
    /// `A2`.
    GetFeatures,
    /// `A3 00 FF`.
    GetLayer,
    /// `A7 01`.
    GetMiscInfo,
    /// `A7 03`.
    GetLayoutLang,
    /// `A7 0B`.
    GetTimers,
    // Read tier, lighting 0xA8.
    /// `A8 01`.
    GetLedCaps,
    /// `A8 03`.
    GetIndicators,
    /// `A8 05`.
    GetLedCount,
    /// `A8 06 row FF FF FF`.
    GetLedRow { row: u8 },
    /// `A8 07`.
    GetPerKeyMode,
    /// `A8 09 start count`.
    GetLedColors { start: u8, count: u8 },
    /// `A8 0B`.
    GetMixInfo,
    /// `A8 0C start count`.
    GetMixRegions { start: u8, count: u8 },
    /// `A8 0E region start count`.
    GetMixSlots { region: u8, start: u8, count: u8 },
    // Read tier, HE 0xA9.
    /// `A9 01`.
    GetHeVersion,
    /// `A9 10`.
    GetProfilesInfo,
    /// `A9 12 profile off_lo off_hi len`, offset little-endian.
    GetProfileBuffer { profile: u8, offset: u16, len: u8 },
    /// `A9 20`.
    GetCurve,
    // Volatile tier.
    /// `A9 11 profile`.
    SelectProfile { profile: u8 },
    /// `A9 14 profile mode actuation press release 01`, keyMode 1 = all keys.
    SetTravelGlobal {
        profile: u8,
        mode: TravelMode,
        actuation: u8,
        press: u8,
        release: u8,
    },
    /// `07 03 id value [sat]`.
    SetLight { value: LightValue },
    // Persistent tier.
    /// `A9 1F profile`.
    SaveProfile { profile: u8 },
    /// `09 03`.
    SaveLighting,
}

fn put(bytes: &[u8]) -> Frame {
    let mut f = [0u8; FRAME_LEN];
    f[..bytes.len()].copy_from_slice(bytes);
    f
}

fn range(field: &'static str, value: u8, (min, max): (u8, u8)) -> Result<(), PolicyError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(PolicyError::OutOfRange {
            field,
            value: u32::from(value),
            min: u32::from(min),
            max: u32::from(max),
        })
    }
}

/// `offset + len` must not pass `limit`.
fn within(field: &'static str, offset: u16, len: u8, limit: u32) -> Result<(), PolicyError> {
    let end = u32::from(offset) + u32::from(len);
    if end <= limit {
        Ok(())
    } else {
        Err(PolicyError::PastEnd { field, end, limit })
    }
}

/// `start + count` must not pass the LED count the device layer supplied.
fn within_leds(field: &'static str, start: u8, count: u8, p: &Policy) -> Result<(), PolicyError> {
    let leds = p
        .led_count
        .ok_or(PolicyError::LimitUnknown { what: "led count" })?;
    let end = u32::from(start) + u32::from(count);
    if end <= u32::from(leds) {
        Ok(())
    } else {
        Err(PolicyError::PastEnd {
            field,
            end,
            limit: u32::from(leds),
        })
    }
}

impl Request {
    /// Which gate this request needs.
    pub fn tier(&self) -> Tier {
        match self {
            Request::SelectProfile { .. }
            | Request::SetTravelGlobal { .. }
            | Request::SetLight { .. } => Tier::Volatile,
            Request::SaveProfile { .. } | Request::SaveLighting => Tier::Persistent,
            _ => Tier::Read,
        }
    }

    fn profile(&self) -> Option<u8> {
        match *self {
            Request::GetProfileBuffer { profile, .. }
            | Request::SelectProfile { profile }
            | Request::SetTravelGlobal { profile, .. }
            | Request::SaveProfile { profile } => Some(profile),
            _ => None,
        }
    }

    /// Range and bounds checks against the sizes the device layer supplied to the policy
    /// (profile count, buffer sizes, matrix, layer count, LED count). None is hardcoded;
    /// an unknown size refuses the request that needs it.
    pub(crate) fn validate(&self, p: &Policy) -> Result<(), PolicyError> {
        if let Some(profile) = self.profile() {
            let Some(count) = p.profile_count else {
                return Err(PolicyError::ProfileCountUnknown);
            };
            if profile >= count {
                return Err(PolicyError::ProfileOutOfRange { profile, count });
            }
        }
        match *self {
            Request::GetKeymapBuffer { offset, len } => {
                if len % 2 != 0 {
                    return Err(PolicyError::Invalid {
                        field: "keymap len",
                        reason: "must be even",
                    });
                }
                range("keymap len", len, (2, VIA_CHUNK_MAX))?;
                let (Some(layers), Some((rows, cols))) = (p.layer_count, p.matrix) else {
                    return Err(PolicyError::LimitUnknown {
                        what: "layer count and matrix",
                    });
                };
                let total = u32::from(layers) * u32::from(rows) * u32::from(cols) * 2;
                within("keymap read", offset, len, total)
            }
            Request::GetMacroBuffer { offset, len } => {
                range("macro len", len, (1, VIA_CHUNK_MAX))?;
                let size = p.macro_buffer_size.ok_or(PolicyError::LimitUnknown {
                    what: "macro buffer size",
                })?;
                within("macro read", offset, len, u32::from(size))
            }
            Request::GetProfileBuffer { offset, len, .. } => {
                range("profile len", len, (1, HE_CHUNK_MAX))?;
                let size = p.profile_buffer_size.ok_or(PolicyError::LimitUnknown {
                    what: "profile buffer size",
                })?;
                within("profile read", offset, len, u32::from(size))
            }
            Request::GetLedRow { row } => {
                let (rows, _) = p
                    .matrix
                    .ok_or(PolicyError::LimitUnknown { what: "matrix" })?;
                if row < rows {
                    Ok(())
                } else {
                    Err(PolicyError::OutOfRange {
                        field: "led row",
                        value: u32::from(row),
                        min: 0,
                        max: u32::from(rows) - 1,
                    })
                }
            }
            Request::GetLedColors { start, count } => {
                range("led count", count, (1, LED_COLORS_MAX))?;
                within_leds("led colors", start, count, p)
            }
            Request::GetMixRegions { start, count } => {
                range("region count", count, (1, MIX_REGIONS_MAX))?;
                within_leds("led regions", start, count, p)
            }
            // Region and start slot are left unbounded on purpose: the read is bounded by
            // count 1..=3 and by the frame size, and it only reads.
            Request::GetMixSlots { count, .. } => range("slot count", count, (1, MIX_SLOTS_MAX)),
            Request::SetTravelGlobal {
                actuation,
                press,
                release,
                ..
            } => {
                range("actuation", actuation, ACTUATION_RANGE)?;
                range("press", press, SENSITIVITY_RANGE)?;
                range("release", release, SENSITIVITY_RANGE)
            }
            Request::SetLight {
                value: LightValue::Effect(effect),
            } => range("effect", effect, (0, EFFECT_MAX)),
            _ => Ok(()),
        }
    }

    /// The wire frame. Crate private: only a `CheckedRequest` exposes bytes.
    pub(crate) fn to_frame(self) -> Frame {
        match self {
            Request::GetViaProtocol => put(&[0x01]),
            Request::GetViaJsonVersion => put(&[0x02, 0x04]),
            Request::GetLight { id } => put(&[0x08, 0x03, id.value_id()]),
            Request::GetLayerCount => put(&[0x11]),
            Request::GetKeymapBuffer { offset, len } => {
                let o = offset.to_be_bytes();
                put(&[0x12, o[0], o[1], len])
            }
            Request::GetMacroCount => put(&[0x0C]),
            Request::GetMacroBufferSize => put(&[0x0D]),
            Request::GetMacroBuffer { offset, len } => {
                let o = offset.to_be_bytes();
                put(&[0x0E, o[0], o[1], len])
            }
            Request::GetKcVersion => put(&[0xA0]),
            Request::GetFirmware => put(&[0xA1]),
            Request::GetFeatures => put(&[0xA2]),
            Request::GetLayer => put(&[0xA3, 0x00, 0xFF]),
            Request::GetMiscInfo => put(&[0xA7, 0x01]),
            Request::GetLayoutLang => put(&[0xA7, 0x03]),
            Request::GetTimers => put(&[0xA7, 0x0B]),
            Request::GetLedCaps => put(&[0xA8, 0x01]),
            Request::GetIndicators => put(&[0xA8, 0x03]),
            Request::GetLedCount => put(&[0xA8, 0x05]),
            Request::GetLedRow { row } => put(&[0xA8, 0x06, row, 0xFF, 0xFF, 0xFF]),
            Request::GetPerKeyMode => put(&[0xA8, 0x07]),
            Request::GetLedColors { start, count } => put(&[0xA8, 0x09, start, count]),
            Request::GetMixInfo => put(&[0xA8, 0x0B]),
            Request::GetMixRegions { start, count } => put(&[0xA8, 0x0C, start, count]),
            Request::GetMixSlots {
                region,
                start,
                count,
            } => put(&[0xA8, 0x0E, region, start, count]),
            Request::GetHeVersion => put(&[0xA9, 0x01]),
            Request::GetProfilesInfo => put(&[0xA9, 0x10]),
            Request::GetProfileBuffer {
                profile,
                offset,
                len,
            } => {
                let o = offset.to_le_bytes();
                put(&[0xA9, 0x12, profile, o[0], o[1], len])
            }
            Request::GetCurve => put(&[0xA9, 0x20]),
            Request::SelectProfile { profile } => put(&[0xA9, 0x11, profile]),
            Request::SetTravelGlobal {
                profile,
                mode,
                actuation,
                press,
                release,
            } => put(&[
                0xA9,
                0x14,
                profile,
                mode.wire(),
                actuation,
                press,
                release,
                0x01,
            ]),
            Request::SetLight { value } => match value {
                LightValue::Brightness(v) | LightValue::Effect(v) | LightValue::Speed(v) => {
                    put(&[0x07, 0x03, value.id().value_id(), v])
                }
                LightValue::Color { hue, saturation } => {
                    put(&[0x07, 0x03, value.id().value_id(), hue, saturation])
                }
            },
            Request::SaveProfile { profile } => put(&[0xA9, 0x1F, profile]),
            Request::SaveLighting => put(&[0x09, 0x03]),
        }
    }

    /// Does `reply` answer this request? Byte 0 echoes the command. Where byte 1 is
    /// reply data (A0, A1, A2, A3, VIA 01, 0C, 0D, 11) only byte 0 is compared. The extra
    /// echoed fields follow the launcher: VIA 12 bytes 0..=2, VIA 0E bytes 0..=3, VIA
    /// 08 03 and 07 03 bytes 0..=2, A9 12 bytes 0..=5. Everything else compares bytes 0
    /// and 1.
    ///
    /// There is no sequence number. A9 11 and other unsolicited reports share the
    /// prefix with a reply, so a match is only as good as the one-frame-in-flight rule.
    pub fn matches(&self, reply: &Frame) -> bool {
        let f = self.to_frame();
        let n = match self {
            Request::GetViaProtocol
            | Request::GetLayerCount
            | Request::GetMacroCount
            | Request::GetMacroBufferSize
            | Request::GetKcVersion
            | Request::GetFirmware
            | Request::GetFeatures
            | Request::GetLayer => 1,
            Request::GetKeymapBuffer { .. }
            | Request::GetLight { .. }
            | Request::SetLight { .. } => 3,
            Request::GetMacroBuffer { .. } => 4,
            Request::GetProfileBuffer { .. } => 6,
            _ => 2,
        };
        reply[..n] == f[..n]
    }

    /// The never-send list, tested on the first two bytes of any slice (also an
    /// over-long one). VIA 0x0A and 0x0B, A7 0x15, every 0xAB, 0xBA.
    pub fn forbidden(bytes: &[u8]) -> Option<PolicyError> {
        let command = *bytes.first()?;
        let sub = bytes.get(1).copied().unwrap_or(0);
        matches!(
            (command, sub),
            (0x0A | 0x0B | 0xAB | 0xBA, _) | (0xA7, 0x15)
        )
        .then_some(PolicyError::Forbidden { command, sub })
    }

    /// Parse a frame back into a typed request. Only allowlisted shapes parse; the
    /// never-send list gives [`PolicyError::Forbidden`], anything else
    /// [`PolicyError::NotAllowlisted`]. Bytes after the used ones must be zero.
    pub fn parse(f: &Frame) -> Result<Request, PolicyError> {
        if let Some(e) = Self::forbidden(f) {
            return Err(e);
        }
        let na = || PolicyError::NotAllowlisted {
            command: f[0],
            sub: f[1],
        };
        let exact = |used: usize, r: Request| {
            if f[used..].iter().all(|&b| b == 0) {
                Ok(r)
            } else {
                Err(PolicyError::TrailingBytes { command: f[0] })
            }
        };
        let be = u16::from_be_bytes([f[1], f[2]]);
        match (f[0], f[1]) {
            (0x01, _) => exact(1, Request::GetViaProtocol),
            (0x02, 0x04) => exact(2, Request::GetViaJsonVersion),
            (0x08, 0x03) => {
                let id = LightId::from_value_id(f[2]).ok_or_else(na)?;
                exact(3, Request::GetLight { id })
            }
            (0x07, 0x03) => {
                let (value, used) = match f[2] {
                    1 => (LightValue::Brightness(f[3]), 4),
                    2 => (LightValue::Effect(f[3]), 4),
                    3 => (LightValue::Speed(f[3]), 4),
                    4 => (
                        LightValue::Color {
                            hue: f[3],
                            saturation: f[4],
                        },
                        5,
                    ),
                    _ => return Err(na()),
                };
                exact(used, Request::SetLight { value })
            }
            (0x09, 0x03) => exact(2, Request::SaveLighting),
            (0x0C, _) => exact(1, Request::GetMacroCount),
            (0x0D, _) => exact(1, Request::GetMacroBufferSize),
            (0x0E, _) => exact(
                4,
                Request::GetMacroBuffer {
                    offset: be,
                    len: f[3],
                },
            ),
            (0x11, _) => exact(1, Request::GetLayerCount),
            (0x12, _) => exact(
                4,
                Request::GetKeymapBuffer {
                    offset: be,
                    len: f[3],
                },
            ),
            (0xA0, _) => exact(1, Request::GetKcVersion),
            (0xA1, _) => exact(1, Request::GetFirmware),
            (0xA2, _) => exact(1, Request::GetFeatures),
            (0xA3, 0x00) if f[2] == 0xFF => exact(3, Request::GetLayer),
            (0xA7, 0x01) => exact(2, Request::GetMiscInfo),
            (0xA7, 0x03) => exact(2, Request::GetLayoutLang),
            (0xA7, 0x0B) => exact(2, Request::GetTimers),
            (0xA8, 0x01) => exact(2, Request::GetLedCaps),
            (0xA8, 0x03) => exact(2, Request::GetIndicators),
            (0xA8, 0x05) => exact(2, Request::GetLedCount),
            (0xA8, 0x06) if f[3..6] == [0xFF; 3] => exact(6, Request::GetLedRow { row: f[2] }),
            (0xA8, 0x07) => exact(2, Request::GetPerKeyMode),
            (0xA8, 0x09) => exact(
                4,
                Request::GetLedColors {
                    start: f[2],
                    count: f[3],
                },
            ),
            (0xA8, 0x0B) => exact(2, Request::GetMixInfo),
            (0xA8, 0x0C) => exact(
                4,
                Request::GetMixRegions {
                    start: f[2],
                    count: f[3],
                },
            ),
            (0xA8, 0x0E) => exact(
                5,
                Request::GetMixSlots {
                    region: f[2],
                    start: f[3],
                    count: f[4],
                },
            ),
            (0xA9, 0x01) => exact(2, Request::GetHeVersion),
            (0xA9, 0x10) => exact(2, Request::GetProfilesInfo),
            (0xA9, 0x11) => exact(3, Request::SelectProfile { profile: f[2] }),
            (0xA9, 0x12) => exact(
                6,
                Request::GetProfileBuffer {
                    profile: f[2],
                    offset: u16::from_le_bytes([f[3], f[4]]),
                    len: f[5],
                },
            ),
            (0xA9, 0x14) if f[7] == 0x01 => {
                let mode = match f[3] {
                    1 => TravelMode::Static,
                    2 => TravelMode::RapidTrigger,
                    other => {
                        return Err(PolicyError::OutOfRange {
                            field: "mode",
                            value: u32::from(other),
                            min: 1,
                            max: 2,
                        });
                    }
                };
                exact(
                    8,
                    Request::SetTravelGlobal {
                        profile: f[2],
                        mode,
                        actuation: f[4],
                        press: f[5],
                        release: f[6],
                    },
                )
            }
            (0xA9, 0x1F) => exact(3, Request::SaveProfile { profile: f[2] }),
            (0xA9, 0x20) => exact(2, Request::GetCurve),
            _ => Err(na()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile error when a variant is added without a sample below.
    fn _every_variant(r: Request) {
        match r {
            Request::GetViaProtocol
            | Request::GetViaJsonVersion
            | Request::GetLight { .. }
            | Request::GetLayerCount
            | Request::GetKeymapBuffer { .. }
            | Request::GetMacroCount
            | Request::GetMacroBufferSize
            | Request::GetMacroBuffer { .. }
            | Request::GetKcVersion
            | Request::GetFirmware
            | Request::GetFeatures
            | Request::GetLayer
            | Request::GetMiscInfo
            | Request::GetLayoutLang
            | Request::GetTimers
            | Request::GetLedCaps
            | Request::GetIndicators
            | Request::GetLedCount
            | Request::GetLedRow { .. }
            | Request::GetPerKeyMode
            | Request::GetLedColors { .. }
            | Request::GetMixInfo
            | Request::GetMixRegions { .. }
            | Request::GetMixSlots { .. }
            | Request::GetHeVersion
            | Request::GetProfilesInfo
            | Request::GetProfileBuffer { .. }
            | Request::GetCurve
            | Request::SelectProfile { .. }
            | Request::SetTravelGlobal { .. }
            | Request::SetLight { .. }
            | Request::SaveProfile { .. }
            | Request::SaveLighting => {}
        }
    }

    #[test]
    fn parse_inverts_to_frame_for_every_variant() {
        let samples = [
            Request::GetViaProtocol,
            Request::GetViaJsonVersion,
            Request::GetLight { id: LightId::Color },
            Request::GetLayerCount,
            Request::GetKeymapBuffer {
                offset: 0x0240,
                len: 28,
            },
            Request::GetMacroCount,
            Request::GetMacroBufferSize,
            Request::GetMacroBuffer {
                offset: 0x0138,
                len: 11,
            },
            Request::GetKcVersion,
            Request::GetFirmware,
            Request::GetFeatures,
            Request::GetLayer,
            Request::GetMiscInfo,
            Request::GetLayoutLang,
            Request::GetTimers,
            Request::GetLedCaps,
            Request::GetIndicators,
            Request::GetLedCount,
            Request::GetLedRow { row: 5 },
            Request::GetPerKeyMode,
            Request::GetLedColors { start: 9, count: 9 },
            Request::GetMixInfo,
            Request::GetMixRegions {
                start: 29,
                count: 29,
            },
            Request::GetMixSlots {
                region: 1,
                start: 3,
                count: 2,
            },
            Request::GetHeVersion,
            Request::GetProfilesInfo,
            Request::GetProfileBuffer {
                profile: 2,
                offset: 0x033C,
                len: 26,
            },
            Request::GetCurve,
            Request::SelectProfile { profile: 1 },
            Request::SetTravelGlobal {
                profile: 1,
                mode: TravelMode::RapidTrigger,
                actuation: 20,
                press: 4,
                release: 5,
            },
            Request::SetLight {
                value: LightValue::Brightness(255),
            },
            Request::SetLight {
                value: LightValue::Effect(24),
            },
            Request::SetLight {
                value: LightValue::Speed(127),
            },
            Request::SetLight {
                value: LightValue::Color {
                    hue: 1,
                    saturation: 2,
                },
            },
            Request::SaveProfile { profile: 2 },
            Request::SaveLighting,
        ];
        for r in samples {
            assert_eq!(Request::parse(&r.to_frame()), Ok(r), "{r:?}");
        }
    }
}
