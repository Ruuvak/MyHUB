//! Decoders for the Keychron system commands A0 to A3 and the misc group A7.
//!
//! Replies of A7, A8 and A9 carry a status byte at index 2 [INF: 0 = OK, 0 on every
//! golden reply]. The decoders expose it as `status` and never treat nonzero as an error.

use serde::Serialize;

use crate::error::DecodeError;
use crate::frame::{Frame, bit, expect};

/// A0 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KcVersion {
    /// Keychron protocol version, byte 1 [OBS 2].
    pub kc_version: u8,
    /// Instruction set, byte 3 [OBS 2].
    pub instruction_set: u8,
}

pub fn decode_kc_version(r: &Frame) -> Result<KcVersion, DecodeError> {
    expect(r, &[0xA0])?;
    Ok(KcVersion {
        kc_version: r[1],
        instruction_set: r[3],
    })
}

/// A1 reply: every nonzero byte from index 1 is a character; zero bytes are skipped,
/// not treated as a terminator [OBS launcher behaviour]. The launcher's optional `v`
/// prefix repair is not applied.
pub fn decode_firmware(r: &Frame) -> Result<String, DecodeError> {
    expect(r, &[0xA1])?;
    Ok(r[1..]
        .iter()
        .filter(|&&b| b != 0)
        .map(|&b| char::from(b))
        .collect())
}

/// A2 reply, V2 layout (kcVersion at most 3) [OBS 2 on the unit].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Features {
    pub f0: u8,
    pub f1: u8,
    pub get_default_layer: bool,
    pub support_le: bool,
    pub support_rf: bool,
    pub support_mock_key: bool,
    pub upload_default_layer: bool,
    pub support_debounce: bool,
    pub support_snap: bool,
    pub support_keychron_rgb: bool,
    pub performance_mode: bool,
    pub bootloader_jump: bool,
    /// Any nonzero F1 marks Quick Start in the launcher [INF].
    pub quick_start: bool,
}

/// The two flag bytes depend on the instruction set: 1 and 3 read bytes 1 and 2, 2 and
/// 4 read bytes 2 and 3. Flag names come from launcher code; most have no consumer
/// there, so their meaning is [INF].
pub fn decode_features(r: &Frame, instruction_set: u8) -> Result<Features, DecodeError> {
    expect(r, &[0xA2])?;
    let (f0, f1) = match instruction_set {
        1 | 3 => (r[1], r[2]),
        2 | 4 => (r[2], r[3]),
        other => return Err(DecodeError::UnsupportedInstructionSet(other)),
    };
    Ok(Features {
        f0,
        f1,
        get_default_layer: bit(f0, 0),
        support_le: bit(f0, 1),
        support_rf: bit(f0, 2),
        support_mock_key: bit(f0, 3),
        upload_default_layer: bit(f0, 4),
        support_debounce: bit(f0, 5),
        support_snap: bit(f0, 6),
        support_keychron_rgb: bit(f0, 7),
        performance_mode: bit(f1, 0),
        bootloader_jump: bit(f1, 3),
        quick_start: f1 != 0,
    })
}

/// A3 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Layer {
    pub default_layer: u8,
    /// `None` when byte 2 is 0xFF (the K2 HE does not report it) [OBS].
    pub current_layer: Option<u8>,
}

pub fn decode_layer(r: &Frame) -> Result<Layer, DecodeError> {
    expect(r, &[0xA3])?;
    Ok(Layer {
        default_layer: r[1],
        current_layer: (r[2] != 0xFF).then_some(r[2]),
    })
}

/// A7 01 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MiscInfo {
    pub status: u8,
    /// LE16 at bytes 3..4 [OBS 2].
    pub version: u16,
    /// Byte 5, unused by the launcher [OBS 0x13, meaning UNK].
    pub flags: u8,
    /// Byte 6 bit 1 [INF].
    pub single_key_reset: bool,
}

pub fn decode_misc_info(r: &Frame) -> Result<MiscInfo, DecodeError> {
    expect(r, &[0xA7, 0x01])?;
    Ok(MiscInfo {
        status: r[2],
        version: u16::from_le_bytes([r[3], r[4]]),
        flags: r[5],
        single_key_reset: bit(r[6], 1),
    })
}

/// A7 03 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LayoutLang {
    pub status: u8,
    /// Keycap legend language index, byte 3 [OBS 0].
    pub index: u8,
}

pub fn decode_layout_lang(r: &Frame) -> Result<LayoutLang, DecodeError> {
    expect(r, &[0xA7, 0x03])?;
    Ok(LayoutLang {
        status: r[2],
        index: r[3],
    })
}

/// A7 0B reply, seconds [OBS 600, 7200, 1800; unit seconds INF].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Timers {
    pub status: u8,
    pub backlight_off_s: u16,
    pub sleep_s: u16,
    pub he_scan_idle_s: u16,
}

pub fn decode_timers(r: &Frame) -> Result<Timers, DecodeError> {
    expect(r, &[0xA7, 0x0B])?;
    Ok(Timers {
        status: r[2],
        backlight_off_s: u16::from_le_bytes([r[3], r[4]]),
        sleep_s: u16::from_le_bytes([r[5], r[6]]),
        he_scan_idle_s: u16::from_le_bytes([r[7], r[8]]),
    })
}
