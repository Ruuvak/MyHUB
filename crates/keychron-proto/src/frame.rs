//! The 32-byte frame.

use crate::error::DecodeError;

/// Frame length on the wire, without the report ID.
pub const FRAME_LEN: usize = 32;

/// One frame. Byte 0 is the command. Unused bytes are zero.
pub type Frame = [u8; FRAME_LEN];

/// Zero-pad `bytes` to a frame. `None` when longer than [`FRAME_LEN`].
pub fn pad(bytes: &[u8]) -> Option<Frame> {
    if bytes.len() > FRAME_LEN {
        return None;
    }
    let mut f = [0u8; FRAME_LEN];
    f[..bytes.len()].copy_from_slice(bytes);
    Some(f)
}

/// Check that the leading bytes of a reply equal `expected`.
pub(crate) fn expect(reply: &Frame, expected: &[u8]) -> Result<(), DecodeError> {
    for (index, &want) in expected.iter().enumerate() {
        if reply[index] != want {
            return Err(DecodeError::WrongReply {
                index,
                expected: want,
                got: reply[index],
            });
        }
    }
    Ok(())
}

pub(crate) fn bit(value: u8, n: u8) -> bool {
    value >> n & 1 == 1
}
