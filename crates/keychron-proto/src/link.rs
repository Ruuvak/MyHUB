//! Decoders for the Keychron Link receiver commands B1, B2 and B3 (see `Request::LinkProtocol`,
//! `LinkState`, `LinkFirmware`). Layouts from the receiver guide, seen on receiver
//! V0.3.0 and V0.3.1 [OBS 2026-10-15]; byte meanings not named there are kept raw.

use serde::Serialize;

use crate::error::DecodeError;
use crate::frame::{Frame, bit, expect};

/// VID of Keychron devices, as the receiver reports the linked keyboard [OBS].
pub const KEYCHRON_VID: u16 = 0x3434;
/// PID of the K2 HE over USB, also what the receiver reports when it is linked [OBS].
pub const K2HE_PID: u16 = 0x0E20;

/// Feature flags, byte 3 of the B1 reply. Bit order is [INF] (launcher indexing); only
/// bit 3 has been seen set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LinkFlags {
    pub raw: u8,
    pub state_notify_over_via: bool,
    pub multi_device: bool,
    pub mouse_driver_over_via: bool,
    pub via_disable_gamepad_input: bool,
}

/// B1 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LinkInfo {
    /// Bytes 1..=2, little-endian [OBS 1].
    pub version: u16,
    pub flags: LinkFlags,
    /// Byte 4, read by the launcher, no known meaning [UNK].
    pub byte4: u8,
}

pub fn decode_link_info(r: &Frame) -> Result<LinkInfo, DecodeError> {
    expect(r, &[0xB1])?;
    Ok(LinkInfo {
        version: u16::from_le_bytes([r[1], r[2]]),
        flags: LinkFlags {
            raw: r[3],
            state_notify_over_via: bit(r[3], 0),
            multi_device: bit(r[3], 1),
            mouse_driver_over_via: bit(r[3], 2),
            via_disable_gamepad_input: bit(r[3], 3),
        },
        byte4: r[4],
    })
}

/// B2 reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LinkState {
    /// VID of the linked device, bytes 2..=3 little-endian [OBS].
    pub linked_vid: u16,
    /// PID of the linked device, bytes 4..=5 little-endian [OBS].
    pub linked_pid: u16,
    /// Byte 6 equals 1 [OBS with the K2 HE awake]. Whether a sleeping keyboard still
    /// reads as connected is [UNK].
    pub connected: bool,
    /// Byte 1, meaning unknown (seen 1) [UNK].
    pub raw_byte1: u8,
}

impl LinkState {
    /// A connected Keychron K2 HE.
    pub fn is_k2he(&self) -> bool {
        self.connected && self.linked_vid == KEYCHRON_VID && self.linked_pid == K2HE_PID
    }
}

pub fn decode_link_state(r: &Frame) -> Result<LinkState, DecodeError> {
    expect(r, &[0xB2])?;
    Ok(LinkState {
        linked_vid: u16::from_le_bytes([r[2], r[3]]),
        linked_pid: u16::from_le_bytes([r[4], r[5]]),
        connected: r[6] == 1,
        raw_byte1: r[1],
    })
}

/// B3 reply: printable ASCII (0x20..=0x7E) from index 1 is kept; zero and other bytes
/// are dropped [launcher skips zeros, OBS].
pub fn decode_link_firmware(r: &Frame) -> Result<String, DecodeError> {
    expect(r, &[0xB3])?;
    Ok(r[1..]
        .iter()
        .filter(|b| (0x20..=0x7E).contains(*b))
        .map(|&b| char::from(b))
        .collect())
}
