//! Session errors.

use keychron_proto::{DecodeError, PolicyError};
use thiserror::Error;

use crate::link::LinkError;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    #[error("link error: {0}")]
    Link(#[from] LinkError),
    /// No matching reply within the reply timeout, after the retries a read gets.
    #[error("no reply within the timeout")]
    Timeout,
    /// The safety policy refused the request. Nothing was written.
    #[error("refused by the safety policy: {0}")]
    Policy(#[from] PolicyError),
    #[error("could not decode a reply: {0}")]
    Decode(#[from] DecodeError),
    /// A report arrived that is not the reply to the pending request and not a known
    /// unsolicited report: another configurator (the Keychron launcher) is talking to
    /// the keyboard. Frames carry no sequence number, so replies cannot be told apart.
    #[error("another program is talking to the keyboard (unexpected report)")]
    ForeignTraffic,
    /// A report matches both the pending request and a late reply of an earlier
    /// exchange, so it cannot be trusted as the answer. No back-off.
    #[error("a reply could not be told from a late reply to an earlier request")]
    AmbiguousReply,
    /// Foreign traffic was seen recently; no request is sent until the back-off ends.
    #[error("backing off after foreign traffic, {remaining_ms} ms left")]
    BackingOff { remaining_ms: u64 },
    /// No raw HID interface of the K2 HE found.
    #[error("no Keychron K2 HE raw HID interface found")]
    NotFound,
    /// The backend reports no usage page and the raw HID interface number is unknown.
    #[error("the OS reports no HID usage page and the raw HID interface number is not known")]
    UsagePageUnavailable,
    /// An input report that is neither 32 bytes nor 33 bytes with a leading 0.
    #[error("input report of {0} bytes is not a 32-byte frame")]
    BadReport(usize),
    /// VIA protocol version other than 12.
    #[error("VIA protocol {0} is not supported (only 12)")]
    UnsupportedVia(u8),
    /// HE command version other than 4.
    #[error("HE command version {0} is not supported (only 4)")]
    UnsupportedHeVersion(u8),
}
