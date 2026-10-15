//! Errors of the reply decoders and read plans.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    #[error("reply byte {index} is {got:#04x}, expected {expected:#04x}")]
    WrongReply { index: usize, expected: u8, got: u8 },
    /// Only HE command version 4 is implemented.
    #[error("HE command version {0} is not supported (only 4)")]
    UnsupportedHeVersion(u8),
    #[error("instruction set {0} has no A2 decoder")]
    UnsupportedInstructionSet(u8),
    #[error("profile buffer is {got} bytes, the layout needs {expected}")]
    LayoutMismatch { expected: usize, got: usize },
    #[error("{field} {got} is above the limit {max}")]
    TooMany {
        field: &'static str,
        got: usize,
        max: usize,
    },
    #[error("invalid read plan: {0}")]
    InvalidPlan(&'static str),
    #[error("expected {expected} replies, got {got}")]
    ReplyCount { expected: usize, got: usize },
    #[error("reply {chunk} does not answer its request")]
    ReplyMismatch { chunk: usize },
}
