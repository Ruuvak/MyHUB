//! Read plans: the chunk requests for the keymap, an HE profile buffer and the macro
//! buffer, and assemblers that join the replies. Plans only build [`Request`]s; each
//! still goes through the policy.

use serde::Serialize;

use crate::error::DecodeError;
use crate::frame::Frame;
use crate::request::{HE_CHUNK_MAX, Request, VIA_CHUNK_MAX};

fn check_replies(requests: &[Request], replies: &[Frame]) -> Result<(), DecodeError> {
    if requests.len() != replies.len() {
        return Err(DecodeError::ReplyCount {
            expected: requests.len(),
            got: replies.len(),
        });
    }
    for (chunk, (req, reply)) in requests.iter().zip(replies).enumerate() {
        if !req.matches(reply) {
            return Err(DecodeError::ReplyMismatch { chunk });
        }
    }
    Ok(())
}

/// Keys per keymap chunk: 14 (28 bytes) [OBS keymap-macros.md section 2].
const KEYS_PER_CHUNK: usize = 14;

/// Keymap of one layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Keymap {
    pub layer: u8,
    pub rows: u8,
    pub cols: u8,
    /// `rows * cols` keycodes, row-major.
    pub keys: Vec<u16>,
}

impl Keymap {
    pub fn get(&self, row: u8, col: u8) -> Option<u16> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.keys
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))
            .copied()
    }

    pub fn row(&self, row: u8) -> Option<&[u16]> {
        if row >= self.rows {
            return None;
        }
        let cols = usize::from(self.cols);
        self.keys
            .get(usize::from(row) * cols..(usize::from(row) + 1) * cols)
    }
}

/// `12 off_hi off_lo len` chunks of one layer: offset = layer * rows * cols * 2 +
/// 28 * chunk, `len` = 2 * min(14, keys left). Keycodes are big-endian from reply byte
/// 4; the padding of a short last chunk is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapPlan {
    layer: u8,
    rows: u8,
    cols: u8,
    requests: Vec<Request>,
}

impl KeymapPlan {
    pub fn new(layer: u8, rows: u8, cols: u8) -> Result<Self, DecodeError> {
        let total = usize::from(rows) * usize::from(cols);
        if total == 0 {
            return Err(DecodeError::InvalidPlan("keymap has no keys"));
        }
        let base = usize::from(layer) * total * 2;
        let mut requests = Vec::new();
        let mut done = 0;
        while done < total {
            let keys = KEYS_PER_CHUNK.min(total - done);
            let offset = u16::try_from(base + done * 2)
                .map_err(|_| DecodeError::InvalidPlan("keymap offset above 65535"))?;
            requests.push(Request::GetKeymapBuffer {
                offset,
                len: (keys * 2) as u8,
            });
            done += keys;
        }
        Ok(KeymapPlan {
            layer,
            rows,
            cols,
            requests,
        })
    }

    pub fn requests(&self) -> &[Request] {
        &self.requests
    }

    /// Replies in request order.
    pub fn assemble(&self, replies: &[Frame]) -> Result<Keymap, DecodeError> {
        check_replies(&self.requests, replies)?;
        let mut keys = Vec::with_capacity(usize::from(self.rows) * usize::from(self.cols));
        for (req, reply) in self.requests.iter().zip(replies) {
            let Request::GetKeymapBuffer { len, .. } = *req else {
                continue;
            };
            keys.extend(
                reply[4..4 + usize::from(len)]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes([c[0], c[1]])),
            );
        }
        Ok(Keymap {
            layer: self.layer,
            rows: self.rows,
            cols: self.cols,
            keys,
        })
    }
}

/// `A9 12` chunks of one HE profile buffer, 26 bytes each, data from reply byte 6.
/// The profile index is checked against the profile count by the policy, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileBufferPlan {
    size: usize,
    requests: Vec<Request>,
}

impl ProfileBufferPlan {
    /// `size` is the buffer size from GET_PROFILES_INFO.
    pub fn new(profile: u8, size: u16) -> Result<Self, DecodeError> {
        if size == 0 {
            return Err(DecodeError::InvalidPlan("profile buffer size is 0"));
        }
        let chunk = usize::from(HE_CHUNK_MAX);
        let size = usize::from(size);
        let requests = (0..size.div_ceil(chunk))
            .map(|i| Request::GetProfileBuffer {
                profile,
                offset: (i * chunk) as u16,
                len: chunk.min(size - i * chunk) as u8,
            })
            .collect();
        Ok(ProfileBufferPlan { size, requests })
    }

    pub fn requests(&self) -> &[Request] {
        &self.requests
    }

    pub fn assemble(&self, replies: &[Frame]) -> Result<Vec<u8>, DecodeError> {
        check_replies(&self.requests, replies)?;
        let mut out = Vec::with_capacity(self.size);
        for (req, reply) in self.requests.iter().zip(replies) {
            if let Request::GetProfileBuffer { len, .. } = *req {
                out.extend_from_slice(&reply[6..6 + usize::from(len)]);
            }
        }
        Ok(out)
    }
}

/// `0E off_hi off_lo len` chunks of the macro buffer, 28 bytes each, data from reply
/// byte 4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroPlan {
    size: usize,
    requests: Vec<Request>,
}

impl MacroPlan {
    /// `size` is the buffer size from VIA 0D.
    pub fn new(size: u16) -> Result<Self, DecodeError> {
        if size == 0 {
            return Err(DecodeError::InvalidPlan("macro buffer size is 0"));
        }
        let chunk = usize::from(VIA_CHUNK_MAX);
        let size = usize::from(size);
        let requests = (0..size.div_ceil(chunk))
            .map(|i| Request::GetMacroBuffer {
                offset: (i * chunk) as u16,
                len: chunk.min(size - i * chunk) as u8,
            })
            .collect();
        Ok(MacroPlan { size, requests })
    }

    pub fn requests(&self) -> &[Request] {
        &self.requests
    }

    pub fn assemble(&self, replies: &[Frame]) -> Result<Vec<u8>, DecodeError> {
        check_replies(&self.requests, replies)?;
        let mut out = Vec::with_capacity(self.size);
        for (req, reply) in self.requests.iter().zip(replies) {
            if let Request::GetMacroBuffer { len, .. } = *req {
                out.extend_from_slice(&reply[4..4 + usize::from(len)]);
            }
        }
        Ok(out)
    }
}
