//! The raw HID pipe abstraction.

use thiserror::Error;

/// Length of one frame, without the report ID.
pub const FRAME_LEN: usize = keychron_proto::FRAME_LEN;
/// Length of one report written: report ID 0 plus the frame.
pub const REPORT_LEN: usize = FRAME_LEN + 1;
/// Size of the read buffer (hidapi may return up to this many bytes).
pub const READ_BUF_LEN: usize = 64;

/// A link-level failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LinkError {
    /// hidapi reported an error.
    #[error("hid error: {0}")]
    Hid(String),
    /// The OS accepted fewer bytes than the report length.
    #[error("short write: {written} of {expected} bytes")]
    ShortWrite { expected: usize, written: usize },
}

/// A raw HID pipe to the keyboard's vendor interface (usage page 0xFF60, usage 0x61).
///
/// This trait performs no safety checks. Only [`Session`](crate::session::Session) may
/// write to it, and only with bytes from `keychron_proto::CheckedRequest::encode`. The
/// real implementation ([`HidapiLink`](crate::hid::HidapiLink)) cannot be constructed
/// outside this crate except through `Session::open`; the trait is public for test mocks
/// and as the session's generic bound.
pub trait RawLink {
    /// Writes one report: byte 0 is the report ID (always 0), bytes 1..33 the frame.
    fn write_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), LinkError>;

    /// Reads one input report into `buf`, waiting up to `timeout_ms` (0 polls without
    /// waiting). Returns the number of bytes read; 0 means nothing arrived. The report
    /// may come back with or without a leading report ID byte; the session normalises it.
    fn read_report(
        &mut self,
        buf: &mut [u8; READ_BUF_LEN],
        timeout_ms: i32,
    ) -> Result<usize, LinkError>;
}
