//! Decoders for the VIA commands in the allowlist.

use crate::error::DecodeError;
use crate::frame::{Frame, expect};
use crate::request::{LightId, LightValue};

/// VIA 01: protocol version, byte 2 [OBS 12].
pub fn decode_protocol(r: &Frame) -> Result<u8, DecodeError> {
    expect(r, &[0x01])?;
    Ok(r[2])
}

/// VIA 11: layer count, byte 1 [OBS 4].
pub fn decode_layer_count(r: &Frame) -> Result<u8, DecodeError> {
    expect(r, &[0x11])?;
    Ok(r[1])
}

/// VIA 0C: macro count, byte 1 [OBS 16].
pub fn decode_macro_count(r: &Frame) -> Result<u8, DecodeError> {
    expect(r, &[0x0C])?;
    Ok(r[1])
}

/// VIA 0D: macro buffer size, big-endian bytes 1..2 [OBS 2027].
pub fn decode_macro_buffer_size(r: &Frame) -> Result<u16, DecodeError> {
    expect(r, &[0x0D])?;
    Ok(u16::from_be_bytes([r[1], r[2]]))
}

/// VIA 08 03 id: the value at byte 3, colour hue at byte 3 and saturation at byte 4.
pub fn decode_light(r: &Frame, id: LightId) -> Result<LightValue, DecodeError> {
    expect(r, &[0x08, 0x03, id.value_id()])?;
    Ok(match id {
        LightId::Brightness => LightValue::Brightness(r[3]),
        LightId::Effect => LightValue::Effect(r[3]),
        LightId::Speed => LightValue::Speed(r[3]),
        LightId::Color => LightValue::Color {
            hue: r[3],
            saturation: r[4],
        },
    })
}
