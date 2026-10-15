//! Decoders for the 0xA8 lighting reads. Byte 2 of these replies is a status byte
//! [INF], exposed as `status`.

use serde::Serialize;

use crate::error::DecodeError;
use crate::frame::{FRAME_LEN, Frame, expect};
use crate::request::{LED_COLORS_MAX, MIX_REGIONS_MAX, MIX_SLOTS_MAX};

/// A8 01 reply. `a8 01 00 01 00` enables both editors on the K2 HE [OBS].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LedCaps {
    /// LE16 at bytes 3..4 equals 1 [INF launcher probe].
    pub per_key: bool,
    /// Byte 2 is 0 and byte 3 is 1 [INF launcher probe].
    pub mix: bool,
}

pub fn decode_led_caps(r: &Frame) -> Result<LedCaps, DecodeError> {
    expect(r, &[0xA8, 0x01])?;
    Ok(LedCaps {
        per_key: u16::from_le_bytes([r[3], r[4]]) == 1,
        mix: u16::from_be_bytes([r[2], r[3]]) == 1,
    })
}

/// A8 03 reply. Indicator bits: 0 Num, 1 Caps, 2 Scroll, 3 Compose, 4 Kana [INF].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Indicators {
    pub status: u8,
    /// Bit i set when indicator i is supported [OBS 0x02, Caps only].
    pub support_mask: u8,
    /// Bit i set when indicator i is disabled [INF].
    pub disable_mask: u8,
    pub hue: u8,
    pub saturation: u8,
    /// Byte 7, not read by the launcher [OBS 0xFF].
    pub value_byte: u8,
}

pub fn decode_indicators(r: &Frame) -> Result<Indicators, DecodeError> {
    expect(r, &[0xA8, 0x03])?;
    Ok(Indicators {
        status: r[2],
        support_mask: r[3],
        disable_mask: r[4],
        hue: r[5],
        saturation: r[6],
        value_byte: r[7],
    })
}

/// A8 05 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LedCount {
    pub status: u8,
    /// Byte 3 [OBS 84].
    pub count: u8,
}

pub fn decode_led_count(r: &Frame) -> Result<LedCount, DecodeError> {
    expect(r, &[0xA8, 0x05])?;
    Ok(LedCount {
        status: r[2],
        count: r[3],
    })
}

/// A8 06 reply: LED index per matrix column of one row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedRow {
    pub status: u8,
    /// One entry per column; `None` where the matrix has no LED (0xFF).
    pub leds: Vec<Option<u8>>,
}

/// Only the first `cols` entries are meaningful; bytes after them are zero, not 0xFF
/// [OBS]. Bytes 3..32 hold at most 29 columns.
pub fn decode_led_row(r: &Frame, cols: usize) -> Result<LedRow, DecodeError> {
    expect(r, &[0xA8, 0x06])?;
    let max = FRAME_LEN - 3;
    if cols > max {
        return Err(DecodeError::TooMany {
            field: "cols",
            got: cols,
            max,
        });
    }
    Ok(LedRow {
        status: r[2],
        leds: r[3..3 + cols]
            .iter()
            .map(|&b| (b != 0xFF).then_some(b))
            .collect(),
    })
}

/// A8 07 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PerKeyMode {
    pub status: u8,
    /// Byte 3: 0 General, 1 Breathing, 2 Single Key Response, 3 Neighboring key
    /// response, 4 Whole Keyboard [INF, labels from launcher i18n].
    pub mode: u8,
}

pub fn decode_per_key_mode(r: &Frame) -> Result<PerKeyMode, DecodeError> {
    expect(r, &[0xA8, 0x07])?;
    Ok(PerKeyMode {
        status: r[2],
        mode: r[3],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Hsv {
    pub hue: u8,
    pub saturation: u8,
    pub value: u8,
}

/// A8 09 reply: `count` triples from byte 3. `count` comes from the request; the
/// reply does not echo start or count [OBS].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedColors {
    pub status: u8,
    pub colors: Vec<Hsv>,
}

pub fn decode_led_colors(r: &Frame, count: u8) -> Result<LedColors, DecodeError> {
    expect(r, &[0xA8, 0x09])?;
    if count > LED_COLORS_MAX {
        return Err(DecodeError::TooMany {
            field: "led count",
            got: usize::from(count),
            max: usize::from(LED_COLORS_MAX),
        });
    }
    Ok(LedColors {
        status: r[2],
        colors: r[3..3 + 3 * usize::from(count)]
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| Hsv {
                hue: t[0],
                saturation: t[1],
                value: t[2],
            })
            .collect(),
    })
}

/// A8 0B reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MixInfo {
    pub status: u8,
    /// Byte 3 [OBS 2].
    pub regions: u8,
    /// Byte 4, effect slots per region [OBS 5].
    pub slots_per_region: u8,
}

pub fn decode_mix_info(r: &Frame) -> Result<MixInfo, DecodeError> {
    expect(r, &[0xA8, 0x0B])?;
    Ok(MixInfo {
        status: r[2],
        regions: r[3],
        slots_per_region: r[4],
    })
}

/// A8 0C reply: region id per LED from byte 3, `count` from the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MixRegions {
    pub status: u8,
    pub regions: Vec<u8>,
}

pub fn decode_mix_regions(r: &Frame, count: u8) -> Result<MixRegions, DecodeError> {
    expect(r, &[0xA8, 0x0C])?;
    if count > MIX_REGIONS_MAX {
        return Err(DecodeError::TooMany {
            field: "region count",
            got: usize::from(count),
            max: usize::from(MIX_REGIONS_MAX),
        });
    }
    Ok(MixRegions {
        status: r[2],
        regions: r[3..3 + usize::from(count)].to_vec(),
    })
}

/// One mix effect slot, 8 bytes on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MixSlot {
    pub effect: u8,
    pub hue: u8,
    pub saturation: u8,
    pub speed: u8,
    pub duration_ms: u32,
}

impl MixSlot {
    /// An all-zero slot is unused; the launcher drops those.
    pub fn is_empty(&self) -> bool {
        *self
            == MixSlot {
                effect: 0,
                hue: 0,
                saturation: 0,
                speed: 0,
                duration_ms: 0,
            }
    }
}

/// A8 0E reply: `count` slots from byte 3. Empty slots are kept so positions hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MixSlots {
    pub status: u8,
    pub slots: Vec<MixSlot>,
}

pub fn decode_mix_slots(r: &Frame, count: u8) -> Result<MixSlots, DecodeError> {
    expect(r, &[0xA8, 0x0E])?;
    if count > MIX_SLOTS_MAX {
        return Err(DecodeError::TooMany {
            field: "slot count",
            got: usize::from(count),
            max: usize::from(MIX_SLOTS_MAX),
        });
    }
    Ok(MixSlots {
        status: r[2],
        slots: r[3..3 + 8 * usize::from(count)]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|s| MixSlot {
                effect: s[0],
                hue: s[1],
                saturation: s[2],
                speed: s[3],
                duration_ms: u32::from_le_bytes([s[4], s[5], s[6], s[7]]),
            })
            .collect(),
    })
}
