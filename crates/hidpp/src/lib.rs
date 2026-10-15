//! HID++ 2.0 protocol: framing, feature table, codecs, CRC, safety policy. No I/O.
//!
//! This crate never touches a device. It encodes and decodes byte buffers and
//! holds the safety allowlist ([`policy::Policy`]) that a transport must consult
//! before every send. Evidence tags in doc comments follow the project spec:
//! `\[OBS\]` observed fact, `\[DER\]` derived, `\[INF\]` unverified inference,
//! `\[UNK\]` unknown.
//!
//! Hard rule of this project: no firmware features, no hidden features. Flash
//! writes exist only as whole-sector writes of user profile sectors behind a
//! separate policy gate ([`onboard_write`], [`policy`]).

pub mod battery;
pub mod crc;
pub mod dpi;
pub mod error;
pub mod feature;
pub mod frame;
pub mod onboard;
pub mod onboard_write;
pub mod policy;
pub mod report_rate;

pub use error::{EncodeError, ParseError};
pub use feature::{FeatureEntry, FeatureId, FeatureTable, FeatureTableError};
pub use frame::{DEFAULT_SW_ID, FrameError, Message, is_reply_to};
pub use policy::{CheckedRequest, Policy, PolicyError};
