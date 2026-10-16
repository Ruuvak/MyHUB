//! Keychron K2 HE raw HID protocol: frames, safety policy, request and reply codecs. No I/O.
//!
//! This crate never touches a device. It builds and decodes 32-byte frames and holds
//! the allowlist ([`policy::Policy`]) that every outgoing request must pass. The report
//! ID (0) belongs to the transport; byte 0 of a frame is the command. There is no
//! sequence number, so a reply is recognised only by [`request::Request::matches`].
//!
//! Safety rules, enforced in [`policy`]: request bytes exist only as a
//! [`policy::CheckedRequest`]; commands outside the v1 allowlist and the never-send list
//! (VIA 0x0A and 0x0B, A7 0x15, every 0xAB, 0xBA) are refused before a byte is built.
//! Writes sit behind two separate gates, volatile (RAM) and persistent (flash).
//!
//! Evidence tags in doc comments: `\[OBS\]` seen on the real keyboard, `\[INF\]` launcher
//! code or inference not yet verified on hardware, `\[UNK\]` gap.

pub mod error;
pub mod frame;
pub mod he;
pub mod keycode;
pub mod lighting;
pub mod link;
pub mod plan;
pub mod policy;
pub mod request;
pub mod system;
pub mod via;

pub use error::DecodeError;
pub use frame::{FRAME_LEN, Frame};
pub use keycode::Keycode;
pub use policy::{CheckedRequest, Policy, PolicyError, Tier};
pub use request::{LightId, LightValue, Request, Target, TravelMode};
