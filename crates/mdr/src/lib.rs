//! Sony MDR headphone control protocol, "TABLE_SET_2" generation (WF-C710N). Pure: no I/O.
//!
//! This crate never touches a device, a socket, a thread or a clock. It frames and
//! parses byte buffers, runs a sans-IO ACK and sequence state machine ([`link`], time
//! is passed in as milliseconds) and holds the safety allowlist ([`policy::Policy`])
//! that every outgoing request must pass. It is the Sony counterpart of the `hidpp`
//! crate. The RFCOMM transport and the device crate live elsewhere.
//!
//! Safety rules of this project, enforced in [`policy`]: request bytes exist only as
//! a [`policy::CheckedRequest`]; firmware update (UPDT), reset settings, every
//! paired-device (PERI) command, all data types except `DATA_MDR` and `DATA_MDR_NO2`
//! and any command not in a tier are denied before a byte is built. Changes that
//! only alter a setting and changes that make the headset drop the link sit behind
//! two separate opt-in gates.
//!
//! Evidence tags in doc comments follow the project spec: `\[OBS\]` observed fact,
//! `\[INF\]` unverified inference, `\[UNK\]` unknown.

pub mod frame;
pub mod function;
pub mod link;
pub mod messages;
pub mod policy;

pub use frame::{
    DataType, Frame, FrameDecoder, FrameError, ReceivedFrame, decode_frame, encode_ack,
};
pub use function::{FunctionEntry, FunctionTable, Table};
pub use link::{Link, LinkEvent, SendError, Tick};
pub use messages::{AlertToken, Incoming, Message, Origin, Request, parse, parse_received};
pub use policy::{CheckedRequest, DeviceLimits, Policy, PolicyError};
