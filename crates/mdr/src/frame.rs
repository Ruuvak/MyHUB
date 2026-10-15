//! Sony MDR data link framing: escaping, checksum, streaming decoder.
//!
//! Wire form: `3E <escaped body> 3C`. Body: `[data type][seq][len u32 BE][payload][checksum]`,
//! checksum = sum of all preceding body bytes modulo 256. Bytes `3C`, `3D`, `3E` inside
//! the body are sent as `3D` followed by `byte & 0xEF`; a receiver ORs the byte after
//! `3D` with `0x10`. Layout from `PROTOCOL.md` section 3 \[OBS on the WF-C710N\].
//!
//! DATA frames cannot be encoded here: wire bytes for requests come only from
//! [`crate::link::Link::send`] (`CheckedRequest::encode` is crate private). ACK frames carry no payload and have their
//! own encoder, [`encode_ack`].

use thiserror::Error;

/// Frame start byte.
pub const START: u8 = 0x3E;
/// Frame end byte.
pub const END: u8 = 0x3C;
/// Escape marker.
pub const ESCAPE: u8 = 0x3D;
/// Largest accepted wire frame, in bytes, start and end included. Longer input is dropped.
pub const MAX_WIRE_FRAME: usize = 4096;

/// Body bytes before the payload: data type, seq, length.
const HEADER_LEN: usize = 6;

/// Data type byte of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    /// Acknowledgement, `0x01`.
    Ack,
    /// Table 1 commands, `0x0C`.
    DataMdr,
    /// Common table, `0x0D`.
    DataCommon,
    /// Table 2 commands, `0x0E`.
    DataMdrNo2,
    /// Table 1 fire and forget, `0x1C`.
    ShotMdr,
    /// Common table fire and forget, `0x1D`.
    ShotCommon,
    /// Table 2 fire and forget, `0x1E`.
    ShotMdrNo2,
    /// Table 1 bulk data (firmware update), `0x2C`.
    LargeDataMdr,
    /// Common table bulk data, `0x2D`.
    LargeDataCommon,
    /// Any other byte.
    Unknown(u8),
}

impl DataType {
    /// Map a wire byte to a data type.
    pub fn from_u8(b: u8) -> Self {
        match b {
            0x01 => Self::Ack,
            0x0C => Self::DataMdr,
            0x0D => Self::DataCommon,
            0x0E => Self::DataMdrNo2,
            0x1C => Self::ShotMdr,
            0x1D => Self::ShotCommon,
            0x1E => Self::ShotMdrNo2,
            0x2C => Self::LargeDataMdr,
            0x2D => Self::LargeDataCommon,
            other => Self::Unknown(other),
        }
    }

    /// The wire byte.
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Ack => 0x01,
            Self::DataMdr => 0x0C,
            Self::DataCommon => 0x0D,
            Self::DataMdrNo2 => 0x0E,
            Self::ShotMdr => 0x1C,
            Self::ShotCommon => 0x1D,
            Self::ShotMdrNo2 => 0x1E,
            Self::LargeDataMdr => 0x2C,
            Self::LargeDataCommon => 0x2D,
            Self::Unknown(b) => b,
        }
    }

    /// True when a receiver must answer with an ACK (`PROTOCOL.md` section 3 table).
    /// Unknown types are not ACKed.
    pub fn needs_ack(self) -> bool {
        matches!(
            self,
            Self::DataMdr
                | Self::DataCommon
                | Self::DataMdrNo2
                | Self::LargeDataMdr
                | Self::LargeDataCommon
        )
    }
}

/// A decoded, checksum verified frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub data_type: DataType,
    pub seq: u8,
    pub payload: Vec<u8>,
}

/// A frame the [`crate::link::Link`] delivered to the application. It has private
/// fields and no public constructor, so one cannot be made from arbitrary bytes. It
/// remembers the link epoch it arrived in. It is not `Clone`: handing it to
/// [`crate::messages::parse_received`] consumes it, so one delivered alert gives one token. Only [`crate::messages::parse_received`]
/// can turn it into a message that carries an [`crate::messages::AlertToken`], and
/// only [`crate::function::FunctionTable::absorb`] fills the function table from it.
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<mdr::ReceivedFrame>();
/// ```
///
/// ```compile_fail
/// let _ = mdr::frame::ReceivedFrame {
///     frame: mdr::Frame { data_type: mdr::DataType::DataMdr, seq: 0, payload: vec![] },
///     epoch: 0,
/// };
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct ReceivedFrame {
    frame: Frame,
    epoch: u64,
}

impl ReceivedFrame {
    pub(crate) fn new(frame: Frame, epoch: u64) -> Self {
        Self { frame, epoch }
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    pub fn data_type(&self) -> DataType {
        self.frame.data_type
    }

    pub fn seq(&self) -> u8 {
        self.frame.seq
    }

    pub fn payload(&self) -> &[u8] {
        &self.frame.payload
    }

    /// The link epoch (see [`crate::link::Link::reset`]) this frame arrived in.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// Why a wire frame was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FrameError {
    #[error("frame is not delimited by 0x3E ... 0x3C")]
    NotDelimited,
    #[error("bad escape sequence")]
    BadEscape,
    #[error("frame body too short")]
    TooShort,
    #[error("length field does not match the body")]
    BadLength,
    #[error("checksum mismatch")]
    BadChecksum,
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

fn escape_into(out: &mut Vec<u8>, body: &[u8]) {
    for &b in body {
        if matches!(b, END | ESCAPE | START) {
            out.push(ESCAPE);
            out.push(b & 0xEF);
        } else {
            out.push(b);
        }
    }
}

/// Build the full wire frame. Crate private on purpose, see the module doc.
pub(crate) fn encode_frame(data_type: DataType, seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(HEADER_LEN + payload.len() + 1);
    body.push(data_type.to_u8());
    body.push(seq);
    body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    body.extend_from_slice(payload);
    body.push(checksum(&body));
    let mut out = Vec::with_capacity(body.len() + 2);
    out.push(START);
    escape_into(&mut out, &body);
    out.push(END);
    out
}

/// Encode an ACK frame. `seq` is the sequence the sender should use next, that is
/// `1 - seq` of the frame being acknowledged. Only the low bit is used.
///
/// `encode_ack(0)` is `3E 01 00 00 00 00 00 01 3C`, `encode_ack(1)` is
/// `3E 01 01 00 00 00 00 02 3C`.
pub fn encode_ack(seq: u8) -> Vec<u8> {
    encode_frame(DataType::Ack, seq & 1, &[])
}

fn unescape(escaped: &[u8]) -> Result<Vec<u8>, FrameError> {
    let mut out = Vec::with_capacity(escaped.len());
    let mut it = escaped.iter();
    while let Some(&b) = it.next() {
        if b == ESCAPE {
            match it.next() {
                Some(&x) if matches!(x, 0x2C..=0x2E) => out.push(x | 0x10),
                _ => return Err(FrameError::BadEscape),
            }
        } else if matches!(b, START | END) {
            return Err(FrameError::BadEscape);
        } else {
            out.push(b);
        }
    }
    Ok(out)
}

fn parse_body(body: &[u8]) -> Result<Frame, FrameError> {
    if body.len() < HEADER_LEN + 1 {
        return Err(FrameError::TooShort);
    }
    let (content, sum) = body.split_at(body.len() - 1);
    let len = u32::from_be_bytes([content[2], content[3], content[4], content[5]]) as usize;
    if len != content.len() - HEADER_LEN {
        return Err(FrameError::BadLength);
    }
    if checksum(content) != sum[0] {
        return Err(FrameError::BadChecksum);
    }
    Ok(Frame {
        data_type: DataType::from_u8(content[0]),
        seq: content[1],
        payload: content[HEADER_LEN..].to_vec(),
    })
}

/// Decode one complete wire frame (`3E ... 3C`).
pub fn decode_frame(wire: &[u8]) -> Result<Frame, FrameError> {
    match wire {
        [START, inner @ .., END] => parse_body(&unescape(inner)?),
        _ => Err(FrameError::NotDelimited),
    }
}

/// Streaming decoder. Feed it any chunking of the byte stream. Bytes outside a frame
/// are skipped, a frame that fails validation is dropped (and counted), and a stray
/// `3E` inside a frame restarts the frame, so the decoder resyncs on the next start byte.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    in_frame: bool,
    dropped: u64,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of frames dropped so far (bad checksum, length, escape or size).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Forget any partial frame.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.in_frame = false;
    }

    /// Push a chunk, return every frame completed by it.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        let mut out = Vec::new();
        for &b in chunk {
            if !self.in_frame {
                if b == START {
                    self.in_frame = true;
                    self.buf.clear();
                }
                continue;
            }
            match b {
                START => {
                    // A start byte inside a frame: the previous one was cut short.
                    if !self.buf.is_empty() {
                        self.dropped += 1;
                    }
                    self.buf.clear();
                }
                END => {
                    match unescape(&self.buf).and_then(|body| parse_body(&body)) {
                        Ok(frame) => out.push(frame),
                        Err(_) => self.dropped += 1,
                    }
                    self.buf.clear();
                    self.in_frame = false;
                }
                _ => {
                    self.buf.push(b);
                    if self.buf.len() + 2 > MAX_WIRE_FRAME {
                        self.dropped += 1;
                        self.buf.clear();
                        self.in_frame = false;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_constants_match_the_app() {
        assert_eq!(
            encode_ack(0),
            [0x3E, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x3C]
        );
        assert_eq!(
            encode_ack(1),
            [0x3E, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x3C]
        );
    }

    #[test]
    fn protocol_info_example_from_the_spec() {
        let wire = encode_frame(DataType::DataMdr, 0, &[0x00, 0x00]);
        assert_eq!(
            wire,
            [
                0x3E, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x0E, 0x3C
            ]
        );
    }

    #[test]
    fn all_three_special_bytes_escape_and_round_trip() {
        let payload = [0x3C, 0x3D, 0x3E, 0x11];
        let wire = encode_frame(DataType::DataMdr, 1, &payload);
        let inner = &wire[1..wire.len() - 1];
        assert!(!inner.contains(&START) && !inner.contains(&END));
        assert!(inner.windows(2).any(|w| w == [0x3D, 0x2C]));
        assert!(inner.windows(2).any(|w| w == [0x3D, 0x2D]));
        assert!(inner.windows(2).any(|w| w == [0x3D, 0x2E]));
        let frame = decode_frame(&wire).expect("decode");
        assert_eq!(frame.payload, payload);
        assert_eq!(frame.seq, 1);
    }

    #[test]
    fn escape_round_trip_every_byte() {
        let all: Vec<u8> = (0..=255).collect();
        let mut esc = Vec::new();
        escape_into(&mut esc, &all);
        assert_eq!(unescape(&esc).unwrap(), all);
    }

    #[test]
    fn checksum_in_the_body_is_escaped_too() {
        let wire = (0..=255u8)
            .map(|b| encode_frame(DataType::DataMdr, 0, &[b]))
            .find(|w| w[w.len() - 3..w.len() - 1] == [0x3D, 0x2C])
            .expect("a checksum of 0x3c exists");
        let frame = decode_frame(&wire).expect("decode");
        assert_eq!(frame.payload.len(), 1);
    }

    #[test]
    fn bad_escape_is_rejected() {
        assert_eq!(
            decode_frame(&[START, 0x0C, 0x3D, 0x41, END]),
            Err(FrameError::BadEscape)
        );
        assert_eq!(
            decode_frame(&[START, 0x0C, 0x3D, END]),
            Err(FrameError::BadEscape)
        );
    }

    #[test]
    fn data_type_round_trip() {
        for b in 0..=255u8 {
            assert_eq!(DataType::from_u8(b).to_u8(), b);
        }
        assert!(DataType::DataMdr.needs_ack());
        assert!(DataType::DataMdrNo2.needs_ack());
        assert!(!DataType::Ack.needs_ack());
        assert!(!DataType::ShotMdr.needs_ack());
        assert!(!DataType::Unknown(0x99).needs_ack());
    }
}
