//! CRC-16/CCITT-FALSE as used for onboard memory sectors.
//!
//! \[OBS\] Polynomial 0x1021, init 0xFFFF, no reflection, no final xor. It covers
//! `sector_size - 2` bytes and is stored big-endian in the last two bytes of the
//! sector. Factory (ROM) sectors store 0x0000 instead.

/// CRC-16/CCITT-FALSE over `data`.
pub fn crc16_ccitt_false(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC over all but the last two bytes of a sector. `None` when the sector is
/// shorter than three bytes.
pub fn sector_crc_computed(sector: &[u8]) -> Option<u16> {
    let body = sector.len().checked_sub(2).filter(|&n| n > 0)?;
    Some(crc16_ccitt_false(&sector[..body]))
}

/// CRC stored big-endian in the last two bytes of a sector. `None` when the
/// sector is shorter than three bytes.
pub fn sector_crc_stored(sector: &[u8]) -> Option<u16> {
    if sector.len() < 3 {
        return None;
    }
    let n = sector.len();
    Some(u16::from_be_bytes([sector[n - 2], sector[n - 1]]))
}

/// `true` when the stored CRC equals the computed one.
pub fn sector_crc_ok(sector: &[u8]) -> bool {
    match (sector_crc_computed(sector), sector_crc_stored(sector)) {
        (Some(c), Some(s)) => c == s,
        _ => false,
    }
}
