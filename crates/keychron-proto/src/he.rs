//! Hall-effect commands 0xA9, HE command version 4 only: reply decoders and the
//! profile buffer parser. Layout from he.md sections 3.2 to 3.5.

use serde::Serialize;

use crate::error::DecodeError;
use crate::frame::{Frame, bit, expect};

/// A9 01 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HeInfo {
    /// Byte 2 [OBS 4].
    pub version: u8,
    /// Byte 4 [OBS 0].
    pub feature_byte: u8,
    pub switch_axis: bool,
    pub reset_single_key: bool,
    pub dkr: bool,
    pub turbo: bool,
    pub gamepad_disable: bool,
    pub support_mechanical: bool,
}

/// Only version 4 is supported; any other version gives
/// [`DecodeError::UnsupportedHeVersion`]. Flag meanings are [INF] from launcher code.
pub fn decode_he_info(r: &Frame) -> Result<HeInfo, DecodeError> {
    expect(r, &[0xA9, 0x01])?;
    if r[2] != 4 {
        return Err(DecodeError::UnsupportedHeVersion(r[2]));
    }
    let f = r[4];
    Ok(HeInfo {
        version: r[2],
        feature_byte: f,
        switch_axis: bit(f, 0),
        reset_single_key: bit(f, 1),
        dkr: bit(f, 2),
        turbo: bit(f, 3),
        gamepad_disable: bit(f, 4),
        support_mechanical: bit(f, 5),
    })
}

/// A9 10 reply [OBS `a9 10 00 03 5c 03 14 14`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProfilesInfo {
    pub current: u8,
    pub count: u8,
    /// Profile buffer size in bytes, LE16 at bytes 4..5.
    pub size: u16,
    pub dks_count: u8,
    pub snap_count: u8,
}

pub fn decode_profiles_info(r: &Frame) -> Result<ProfilesInfo, DecodeError> {
    expect(r, &[0xA9, 0x10])?;
    Ok(ProfilesInfo {
        current: r[2],
        count: r[3],
        size: u16::from_le_bytes([r[4], r[5]]),
        dks_count: r[6],
        snap_count: r[7],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CurvePoint {
    pub x: u8,
    pub y: u8,
}

/// A9 20 reply: four points from byte 2 [OBS (0,0) (10,31) (30,95) (40,127); units INF].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Curve {
    pub points: [CurvePoint; 4],
}

pub fn decode_curve(r: &Frame) -> Result<Curve, DecodeError> {
    expect(r, &[0xA9, 0x20])?;
    let p = |i: usize| CurvePoint {
        x: r[2 + 2 * i],
        y: r[3 + 2 * i],
    };
    Ok(Curve {
        points: [p(0), p(1), p(2), p(3)],
    })
}

/// Key work mode, bits 0..1 of a key word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum KeyMode {
    /// Follow the profile's global word.
    Global,
    Static,
    RapidTrigger,
    Unknown(u8),
}

/// Advance mode, bits 20..23 of a key word. Stored numbers, not the SET_ADVANCE_MODE
/// numbers (DKS is 1 on write and 3 stored) [OBS for DKS 3 and analog 4; the rest INF].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AdvanceMode {
    None,
    Dks,
    Analog,
    Toggle,
    Dkr,
    Turbo,
    Unknown(u8),
}

/// A 32-bit little-endian key word, also used for the profile's global word. Fields are
/// counted from the least significant bit: mode 0..1, distance 2..7, press 8..13,
/// release 14..19, advance mode 20..23, index 24..31. Checked against the golden global
/// word 0x00010451 [OBS].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KeyWord {
    pub raw: u32,
    pub mode: KeyMode,
    /// Actuation point, 0.1 mm.
    pub distance: u8,
    /// Rapid trigger press sensitivity, 0.1 mm.
    pub press: u8,
    /// Rapid trigger release sensitivity, 0.1 mm.
    pub release: u8,
    pub advance_mode: AdvanceMode,
    /// DKS slot for DKS, analog id for analog, repeat count for DKR and Turbo.
    pub index: u8,
}

impl KeyWord {
    pub fn from_u32(raw: u32) -> Self {
        let mode = match raw & 3 {
            0 => KeyMode::Global,
            1 => KeyMode::Static,
            2 => KeyMode::RapidTrigger,
            n => KeyMode::Unknown(n as u8),
        };
        let advance_mode = match (raw >> 20) & 0xF {
            0 => AdvanceMode::None,
            3 => AdvanceMode::Dks,
            4 => AdvanceMode::Analog,
            5 => AdvanceMode::Toggle,
            7 => AdvanceMode::Dkr,
            8 => AdvanceMode::Turbo,
            n => AdvanceMode::Unknown(n as u8),
        };
        KeyWord {
            raw,
            mode,
            distance: ((raw >> 2) & 63) as u8,
            press: ((raw >> 8) & 63) as u8,
            release: ((raw >> 14) & 63) as u8,
            advance_mode,
            index: (raw >> 24) as u8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KeyPos {
    pub row: u8,
    pub col: u8,
}

/// One 19-byte DKS slot. Travel unit 0.1 mm [INF], stage meaning of the four travel
/// points and action nibbles [UNK]; the unpack formulas are confirmed byte for byte by
/// write test T1 [OBS].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DksSlot {
    /// False when the 24-bit travel word is 0.
    pub used: bool,
    /// Four 6-bit travel points in the order of the SET frame d0, d1, d2, d3
    /// (bits 0..5, 12..17, 18..23, 6..11 of the travel word).
    pub travel: [u8; 4],
    /// Four keycodes, little-endian on the wire.
    pub keycodes: [u16; 4],
    /// Per keycode, one nibble per travel stage.
    pub actions: [[u8; 4]; 4],
    /// The key whose word points at this slot, when there is one.
    pub owner: Option<KeyPos>,
}

/// Snap tap pair type, byte 2 of a slot [INF launcher WebMCP: 2 to 6].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SnapType {
    SnapClick,
    LastKeyPriority,
    AbsoluteFirst,
    AbsoluteSecond,
    Neutral,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SnapSlot {
    /// False when bytes 0 and 1 are both zero. A pair that includes key (0,0) twice is
    /// indistinguishable from empty [INF].
    pub used: bool,
    pub key1: KeyPos,
    pub key2: KeyPos,
    pub kind: SnapType,
}

/// A parsed version 4 profile buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileBuffer {
    pub rows: u8,
    pub cols: u8,
    pub global: KeyWord,
    /// `rows * cols` key words, row-major (matrix positions without a key included).
    pub keys: Vec<KeyWord>,
    pub dks: Vec<DksSlot>,
    pub snap: Vec<SnapSlot>,
    /// Profile name, UTF-16LE up to the first 00 00 [OBS firmware stores bytes as sent].
    pub name: String,
}

const NAME_LEN: usize = 32;
const DKS_LEN: usize = 19;
const SNAP_LEN: usize = 3;

fn word(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn snap_key(b: u8) -> KeyPos {
    KeyPos {
        row: b & 7,
        col: b >> 3,
    }
}

impl ProfileBuffer {
    /// Total size the v4 layout needs: `4 + 4*rows*cols + 19*dks + 3*snap + 32`.
    pub fn layout_size(rows: u8, cols: u8, dks_count: usize, snap_count: usize) -> usize {
        4 + 4 * usize::from(rows) * usize::from(cols)
            + DKS_LEN * dks_count
            + SNAP_LEN * snap_count
            + NAME_LEN
    }

    /// Parse. `rows` and `cols` come from the keyboard definition, the slot counts from
    /// GET_PROFILES_INFO. A size that does not equal the layout is an error.
    pub fn parse(
        bytes: &[u8],
        rows: u8,
        cols: u8,
        dks_count: u8,
        snap_count: u8,
    ) -> Result<Self, DecodeError> {
        let dks_count = usize::from(dks_count);
        let snap_count = usize::from(snap_count);
        let expected = Self::layout_size(rows, cols, dks_count, snap_count);
        if bytes.len() != expected {
            return Err(DecodeError::LayoutMismatch {
                expected,
                got: bytes.len(),
            });
        }
        let key_count = usize::from(rows) * usize::from(cols);
        let keys_end = 4 + 4 * key_count;
        let dks_end = keys_end + DKS_LEN * dks_count;
        let snap_end = dks_end + SNAP_LEN * snap_count;

        let keys: Vec<KeyWord> = bytes[4..keys_end]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| KeyWord::from_u32(word(c)))
            .collect();

        let dks = bytes[keys_end..dks_end]
            .as_chunks::<DKS_LEN>()
            .0
            .iter()
            .enumerate()
            .map(|(slot, s)| {
                let t = u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16;
                let owner = keys.iter().enumerate().find_map(|(i, k)| {
                    (k.advance_mode == AdvanceMode::Dks && usize::from(k.index) == slot).then(
                        || KeyPos {
                            row: (i / usize::from(cols)) as u8,
                            col: (i % usize::from(cols)) as u8,
                        },
                    )
                });
                let mut keycodes = [0u16; 4];
                let mut actions = [[0u8; 4]; 4];
                for k in 0..4 {
                    keycodes[k] = u16::from_le_bytes([s[3 + 2 * k], s[4 + 2 * k]]);
                    let (p0, p1) = (s[11 + 2 * k], s[12 + 2 * k]);
                    actions[k] = [p0 & 15, p1 & 15, p1 >> 4, p0 >> 4];
                }
                DksSlot {
                    used: t != 0,
                    travel: [
                        (t & 63) as u8,
                        ((t >> 12) & 63) as u8,
                        ((t >> 18) & 63) as u8,
                        ((t >> 6) & 63) as u8,
                    ],
                    keycodes,
                    actions,
                    owner,
                }
            })
            .collect();

        let snap = bytes[dks_end..snap_end]
            .as_chunks::<SNAP_LEN>()
            .0
            .iter()
            .map(|s| SnapSlot {
                used: s[0] != 0 || s[1] != 0,
                key1: snap_key(s[0]),
                key2: snap_key(s[1]),
                kind: match s[2] {
                    2 => SnapType::SnapClick,
                    3 => SnapType::LastKeyPriority,
                    4 => SnapType::AbsoluteFirst,
                    5 => SnapType::AbsoluteSecond,
                    6 => SnapType::Neutral,
                    n => SnapType::Unknown(n),
                },
            })
            .collect();

        let units: Vec<u16> = bytes[snap_end..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();

        Ok(ProfileBuffer {
            rows,
            cols,
            global: KeyWord::from_u32(word(&bytes[..4])),
            keys,
            dks,
            snap,
            name: String::from_utf16_lossy(&units),
        })
    }

    /// The key word at a matrix position.
    pub fn key(&self, row: u8, col: u8) -> Option<&KeyWord> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.keys
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))
    }
}
