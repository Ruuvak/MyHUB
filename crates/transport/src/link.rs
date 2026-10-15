//! The raw HID pipe abstraction.

use thiserror::Error;

/// Length of every report written (HID++ long report).
pub const WRITE_LEN: usize = 20;
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

/// A bidirectional HID report pipe to the receiver's HID++ interface.
///
/// This trait performs no safety checks. Only
/// [`HidppSession`](crate::session::HidppSession) may write to it, and only
/// with bytes from `hidpp::CheckedRequest::encode`. The real implementation is
/// not constructible outside this crate; the trait is public for test mocks
/// and as the session's generic bound.
pub trait HidLink {
    /// Writes one 20 byte long report (byte 0 is the report ID 0x11).
    fn write_report(&mut self, report: &[u8; WRITE_LEN]) -> Result<(), LinkError>;

    /// Reads one report into `buf`, waiting up to `timeout_ms` (0 polls
    /// without waiting). Returns the number of bytes read; 0 means nothing
    /// arrived. Reports of all collections the link reads are merged.
    fn read_report(
        &mut self,
        buf: &mut [u8; READ_BUF_LEN],
        timeout_ms: i32,
    ) -> Result<usize, LinkError>;

    /// `false` when the link cannot see short (0x10) reports, so HID++ 1.0
    /// messages such as the 0x41 connection notification may be missed. A
    /// link that reads one node for both report IDs returns `true`.
    fn short_reports_available(&self) -> bool {
        true
    }
}
