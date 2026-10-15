//! Onboard profiles (0x8100): read and parse.
//!
//! There is deliberately no general sector serializer. The only write support
//! is [`crate::onboard_write`], which patches known fields over the raw bytes
//! read from the device. Writes are gated by [`crate::policy`].
//!
//! Only profile format 3 (layout A) is verified on the device; every other
//! format is refused by [`ProfileSector::parse`].

use serde::Serialize;

use crate::crc;
use crate::error::ParseError;

pub const FN_GET_INFO: u8 = 0;
pub const FN_SET_ONBOARD_MODE: u8 = 1;
pub const FN_IS_ONBOARD_MODE: u8 = 2;
pub const FN_GET_ACTIVE_PROFILE: u8 = 4;
pub const FN_READ_DATA: u8 = 5;

/// Directory sector number.
pub const DIRECTORY_SECTOR: u16 = 0x0000;
/// The only profile format with a verified layout (layout A).
pub const SUPPORTED_PROFILE_FORMAT: u8 = 3;
/// ReadData reply size in bytes.
pub const CHUNK_LEN: usize = 16;

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

/// Decoded GetOnboardProfilesInfo (fn 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OnboardInfo {
    pub memory_model: u8,
    /// Profile format ID. 3 selects layout A.
    pub profile_format: u8,
    pub macro_format: u8,
    pub profile_count: u8,
    pub rom_profile_count: u8,
    pub button_count: u8,
    pub sector_count: u8,
    /// Sector size in bytes, BE16 at bytes 7..8.
    pub sector_size: u16,
    /// Byte 9, raw flags. Fixture 0x0A.
    pub flags: u8,
}

impl OnboardInfo {
    /// Parses a GetOnboardProfilesInfo reply payload.
    pub fn parse(payload: &[u8]) -> Result<Self, ParseError> {
        need(payload, 10)?;
        Ok(Self {
            memory_model: payload[0],
            profile_format: payload[1],
            macro_format: payload[2],
            profile_count: payload[3],
            rom_profile_count: payload[4],
            button_count: payload[5],
            sector_count: payload[6],
            sector_size: u16::from_be_bytes([payload[7], payload[8]]),
            flags: payload[9],
        })
    }
}

/// Request parameters for ReadData: sector BE16, offset BE16.
pub fn read_data_params(sector: u16, offset: u16) -> [u8; 16] {
    let mut params = [0u8; 16];
    params[..2].copy_from_slice(&sector.to_be_bytes());
    params[2..4].copy_from_slice(&offset.to_be_bytes());
    params
}

/// Offsets of the ReadData requests needed to read a whole sector: 0, 16, ...
/// with the last one clamped to `size - 16` so no read crosses the sector end
/// (same as the probe: `min(off, size - 16)`). For size 255 the plan ends at
/// offset 239.
pub fn chunk_plan(sector_size: u16) -> Result<Vec<u16>, ParseError> {
    let size = usize::from(sector_size);
    if size < CHUNK_LEN {
        return Err(ParseError::SectorTooSmall {
            size,
            need: CHUNK_LEN,
        });
    }
    let last = size - CHUNK_LEN;
    let mut offsets = Vec::with_capacity(size.div_ceil(CHUNK_LEN));
    let mut off = 0usize;
    while off < size {
        // `min` keeps the value <= size - 16 <= u16::MAX.
        offsets.push(off.min(last) as u16);
        off += CHUNK_LEN;
    }
    Ok(offsets)
}

/// Builds a full sector from 16-byte ReadData replies.
#[derive(Debug, Clone)]
pub struct SectorAssembler {
    data: Vec<u8>,
    covered: Vec<bool>,
}

impl SectorAssembler {
    pub fn new(sector_size: u16) -> Result<Self, ParseError> {
        let size = usize::from(sector_size);
        if size < CHUNK_LEN {
            return Err(ParseError::SectorTooSmall {
                size,
                need: CHUNK_LEN,
            });
        }
        Ok(Self {
            data: vec![0; size],
            covered: vec![false; size],
        })
    }

    /// Stores the first 16 bytes of a ReadData reply payload at `offset`.
    pub fn push_chunk(&mut self, offset: u16, reply: &[u8]) -> Result<(), ParseError> {
        need(reply, CHUNK_LEN)?;
        let start = usize::from(offset);
        let end = start + CHUNK_LEN;
        if end > self.data.len() {
            return Err(ParseError::ChunkOutOfRange {
                offset: start,
                size: self.data.len(),
            });
        }
        self.data[start..end].copy_from_slice(&reply[..CHUNK_LEN]);
        self.covered[start..end].fill(true);
        Ok(())
    }

    /// `true` when every byte of the sector has been read.
    pub fn is_complete(&self) -> bool {
        self.covered.iter().all(|&c| c)
    }

    /// Returns the sector bytes, or an error when bytes are still missing.
    pub fn finish(self) -> Result<Vec<u8>, ParseError> {
        let missing = self.covered.iter().filter(|&&c| !c).count();
        if missing > 0 {
            return Err(ParseError::SectorIncomplete { missing });
        }
        Ok(self.data)
    }
}

/// One directory entry (sector 0x0000).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DirectoryEntry {
    pub sector: u16,
    pub enabled: bool,
}

/// Directory sector contents: 4-byte entries `{sector BE16, enabled, 0xFF}`
/// terminated by sector 0xFFFF \[OBS\].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Directory {
    pub entries: Vec<DirectoryEntry>,
}

impl Directory {
    /// Parses the bytes of sector 0x0000.
    pub fn parse(sector: &[u8]) -> Result<Self, ParseError> {
        need(sector, 4)?;
        let mut entries = Vec::new();
        for chunk in sector.as_chunks::<4>().0 {
            let id = u16::from_be_bytes([chunk[0], chunk[1]]);
            if id == 0xFFFF {
                break;
            }
            entries.push(DirectoryEntry {
                sector: id,
                enabled: chunk[2] != 0,
            });
        }
        Ok(Self { entries })
    }

    /// Sectors of enabled profiles, in directory order.
    pub fn enabled_sectors(&self) -> Vec<u16> {
        self.entries
            .iter()
            .filter(|e| e.enabled)
            .map(|e| e.sector)
            .collect()
    }
}

/// Special function codes of `90 nn 00 00` bindings. Codes 01, 02, 03, 04, 07,
/// 0A, 0B are \[OBS\]. Anything else, including 0x10 and 0x11, is \[UNK\] and kept
/// as [`SpecialFunction::Other`], never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SpecialFunction {
    WheelLeft,
    WheelRight,
    DpiUp,
    DpiDown,
    DpiShift,
    ProfileCycle,
    GShift,
    Other(u8),
}

impl SpecialFunction {
    pub fn from_code(code: u8) -> Self {
        match code {
            0x01 => Self::WheelLeft,
            0x02 => Self::WheelRight,
            0x03 => Self::DpiUp,
            0x04 => Self::DpiDown,
            0x07 => Self::DpiShift,
            0x0A => Self::ProfileCycle,
            0x0B => Self::GShift,
            other => Self::Other(other),
        }
    }
}

/// A 4-byte button binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Binding {
    /// `80 01 hi lo`: mouse button bitmask BE16 (0x0001 left, 0x0002 right,
    /// 0x0004 middle, 0x0008 back, 0x0010 forward).
    MouseButtons(u16),
    /// `80 02 mod usage`: modifier bitmask and HID keyboard usage.
    Keyboard { modifiers: u8, usage: u8 },
    /// `80 03 hi lo`: HID consumer usage BE16.
    Consumer(u16),
    /// `90 nn 00 00`: special function.
    Special(SpecialFunction),
    /// `FF FF FF FF`.
    Unassigned,
    /// Anything else, raw.
    Unknown([u8; 4]),
}

impl Binding {
    /// Decodes 4 binding bytes.
    pub fn decode(bytes: [u8; 4]) -> Self {
        match bytes {
            [0xFF, 0xFF, 0xFF, 0xFF] => Self::Unassigned,
            [0x80, 0x01, hi, lo] => Self::MouseButtons(u16::from_be_bytes([hi, lo])),
            [0x80, 0x02, modifiers, usage] => Self::Keyboard { modifiers, usage },
            [0x80, 0x03, hi, lo] => Self::Consumer(u16::from_be_bytes([hi, lo])),
            [0x90, code, 0x00, 0x00] => Self::Special(SpecialFunction::from_code(code)),
            other => Self::Unknown(other),
        }
    }
}

/// Name of button slot `slot` (0-based): `G{slot+1}` while `slot < button_count`.
/// Slots beyond the device's button count (layout A has 16 slots, the G502 X
/// reports 11 buttons) have no user-facing name.
pub fn slot_name(slot: usize, button_count: u8) -> Option<String> {
    (slot < usize::from(button_count)).then(|| format!("G{}", slot + 1))
}

/// Layout A offsets \[OBS\] (format 3), button slots corrected on the fixture:
/// 16 slots per layer, not 12.
pub mod layout_a {
    pub const REPORT_RATE: usize = 0x00;
    pub const DEFAULT_DPI_INDEX: usize = 0x01;
    pub const SHIFT_DPI_INDEX: usize = 0x02;
    pub const DPI_LEVELS: usize = 0x03;
    pub const DPI_LEVEL_COUNT: usize = 5;
    pub const COLOR: usize = 0x0D;
    pub const POWER_MODE: usize = 0x10;
    pub const ANGLE_SNAPPING: usize = 0x11;
    pub const WRITE_COUNTER: usize = 0x12;
    pub const RESERVED: usize = 0x14;
    pub const RESERVED_LEN: usize = 8;
    pub const POWER_SAVE_TIMEOUT: usize = 0x1C;
    pub const POWER_OFF_TIMEOUT: usize = 0x1E;
    pub const BUTTONS: usize = 0x20;
    pub const GSHIFT_BUTTONS: usize = 0x60;
    pub const BUTTON_SLOTS: usize = 16;
    pub const NAME: usize = 0xA0;
    pub const NAME_UNITS: usize = 24;
    pub const LED_ZONES: usize = 0xD0;
    pub const LED_ZONE_COUNT: usize = 4;
    pub const LED_ZONE_LEN: usize = 11;
    pub const FLAG: usize = 0xFC;
    pub const TRAILER: usize = 0xFD;
    /// Minimum sector size: flag byte at 0xFC plus the 2 CRC bytes.
    pub const MIN_SECTOR_SIZE: usize = 0xFF;
}

/// A parsed layout A profile sector.
///
/// All raw bytes are kept in [`ProfileSector::raw`]; any later edit must patch
/// known fields over those bytes so unknown bytes stay as read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileSector {
    /// Report rate byte: interval in ms (1 = 1000 Hz, 2 = 500 Hz).
    pub report_rate: u8,
    pub default_dpi_index: u8,
    pub shift_dpi_index: u8,
    /// Five DPI levels, u16 little-endian \[OBS\].
    pub dpi_levels: [u16; 5],
    /// RGB color.
    pub color: [u8; 3],
    /// Raw power mode byte. Meaning \[UNK\] (fixture 0xFF).
    pub power_mode: u8,
    /// Raw angle snapping byte (fixture 0).
    pub angle_snapping: u8,
    /// Write counter, raw (fixture 0xFFFF).
    pub write_counter: [u8; 2],
    /// Bytes 0x14..0x1C, raw.
    pub reserved: [u8; 8],
    /// Power save timeout, u16 little-endian (fixture 60).
    pub power_save_timeout: u16,
    /// Power off timeout, u16 little-endian (fixture 300).
    pub power_off_timeout: u16,
    /// 16 button slots at 0x20..0x60.
    pub buttons: [Binding; 16],
    /// 16 G-shift slots at 0x60..0xA0.
    pub gshift_buttons: [Binding; 16],
    /// Profile name (24 UTF-16LE units at 0xA0, 0x0000 terminated). `None` when
    /// the first unit is 0xFFFF (unset).
    pub name: Option<String>,
    /// LED effect zones at 0xD0, 0xDB, 0xE6, 0xF1 (11 bytes each), raw.
    /// Semantics \[UNK\].
    pub led_zones: [[u8; 11]; 4],
    /// Byte 0xFC, raw flag.
    pub flag: u8,
    /// Bytes from 0xFD up to the CRC, raw. Empty for sector size 255, where
    /// bytes 0xFD..0xFE are the CRC.
    pub trailer: Vec<u8>,
    /// CRC stored in the last two bytes (BE).
    pub stored_crc: u16,
    /// CRC-16/CCITT-FALSE computed over all bytes but the last two.
    pub computed_crc: u16,
    /// `stored_crc == computed_crc`.
    pub crc_ok: bool,
    /// The full sector as read.
    pub raw: Vec<u8>,
}

fn le16(bytes: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([bytes[off], bytes[off + 1]])
}

fn bindings(bytes: &[u8], base: usize) -> [Binding; 16] {
    std::array::from_fn(|i| {
        let o = base + i * 4;
        Binding::decode([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
    })
}

fn decode_name(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    if units.first() == Some(&0xFFFF) {
        return None;
    }
    let end = units
        .iter()
        .position(|&u| u == 0x0000 || u == 0xFFFF)
        .unwrap_or(units.len());
    Some(String::from_utf16_lossy(&units[..end]))
}

impl ProfileSector {
    /// Parses a profile sector. Refuses unless `info.profile_format == 3` and
    /// `bytes.len() == info.sector_size`.
    pub fn parse(bytes: &[u8], info: &OnboardInfo) -> Result<Self, ParseError> {
        use layout_a as a;

        if info.profile_format != SUPPORTED_PROFILE_FORMAT {
            return Err(ParseError::UnsupportedProfileFormat(info.profile_format));
        }
        let expected = usize::from(info.sector_size);
        if bytes.len() != expected {
            return Err(ParseError::SectorLength {
                expected,
                got: bytes.len(),
            });
        }
        if bytes.len() < a::MIN_SECTOR_SIZE {
            return Err(ParseError::SectorTooSmall {
                size: bytes.len(),
                need: a::MIN_SECTOR_SIZE,
            });
        }

        let dpi_levels = std::array::from_fn(|i| le16(bytes, a::DPI_LEVELS + i * 2));
        let led_zones = std::array::from_fn(|z| {
            let o = a::LED_ZONES + z * a::LED_ZONE_LEN;
            let mut zone = [0u8; 11];
            zone.copy_from_slice(&bytes[o..o + a::LED_ZONE_LEN]);
            zone
        });
        let mut color = [0u8; 3];
        color.copy_from_slice(&bytes[a::COLOR..a::COLOR + 3]);
        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&bytes[a::RESERVED..a::RESERVED + a::RESERVED_LEN]);

        let crc_body = bytes.len() - 2;
        let stored_crc = crc::sector_crc_stored(bytes).unwrap_or(0);
        let computed_crc = crc::sector_crc_computed(bytes).unwrap_or(0);

        Ok(Self {
            report_rate: bytes[a::REPORT_RATE],
            default_dpi_index: bytes[a::DEFAULT_DPI_INDEX],
            shift_dpi_index: bytes[a::SHIFT_DPI_INDEX],
            dpi_levels,
            color,
            power_mode: bytes[a::POWER_MODE],
            angle_snapping: bytes[a::ANGLE_SNAPPING],
            write_counter: [bytes[a::WRITE_COUNTER], bytes[a::WRITE_COUNTER + 1]],
            reserved,
            power_save_timeout: le16(bytes, a::POWER_SAVE_TIMEOUT),
            power_off_timeout: le16(bytes, a::POWER_OFF_TIMEOUT),
            buttons: bindings(bytes, a::BUTTONS),
            gshift_buttons: bindings(bytes, a::GSHIFT_BUTTONS),
            name: decode_name(&bytes[a::NAME..a::NAME + a::NAME_UNITS * 2]),
            led_zones,
            flag: bytes[a::FLAG],
            trailer: bytes[a::TRAILER..crc_body].to_vec(),
            stored_crc,
            computed_crc,
            crc_ok: stored_crc == computed_crc,
            raw: bytes.to_vec(),
        })
    }
}
