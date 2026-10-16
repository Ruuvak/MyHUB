//! Reads from the Keychron Link receiver (B1, B2, B3), over a [`Session::open_receiver`]
//! session. Read only. B5 (gamepad report), BC (state notify, times out) and BA (receiver
//! DFU) are never sent, and no keyboard frame can go over a receiver session.
//!
//! [OBS] Over the receiver every keyboard frame times out, so configuration needs the USB
//! cable. [UNK] Whether B2 reports a sleeping keyboard as connected.

use keychron_proto::Request;
use keychron_proto::link::{
    LinkInfo, LinkState, decode_link_firmware, decode_link_info, decode_link_state,
};
use serde::Serialize;

use crate::error::SessionError;
use crate::link::RawLink;
use crate::session::Session;
use crate::snapshot::{SectionNote, ask, soft};

/// What [`read_link`] saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkSnapshot {
    /// B1; `None` with a note when it timed out or did not decode.
    pub info: Option<LinkInfo>,
    /// B2, required.
    pub state: LinkState,
    /// B3; `None` with a note when it timed out or did not decode.
    pub firmware: Option<String>,
    /// Frames written, resends included.
    pub requests: u32,
    pub notes: Vec<SectionNote>,
}

/// One B2 read, for polling [OBS: the receiver gives no change notification].
pub fn read_link_state<L: RawLink>(s: &mut Session<L>) -> Result<LinkState, SessionError> {
    ask(s, Request::LinkState, decode_link_state)
}

/// Cost: when the receiver does not answer (asleep), each of the three reads waits the
/// reply timeout three times (one try, two resends), about 9 s in total. Polling (S5b)
/// should use [`read_link_state`] only.
///
/// B1, B2 and B3 once, in that order. B2 is required: a failure of it fails the read.
/// A timeout or decode failure of B1 or B3 gives `None` and a note. Foreign traffic,
/// back-off, link, policy and target errors always fail the read.
pub fn read_link<L: RawLink>(s: &mut Session<L>) -> Result<LinkSnapshot, SessionError> {
    let sent_before = s.frames_sent();
    let mut notes = Vec::new();
    let info = soft(
        &mut notes,
        "link protocol",
        ask(s, Request::LinkProtocol, decode_link_info),
    )?;
    let state = read_link_state(s)?;
    let firmware = soft(
        &mut notes,
        "link firmware",
        ask(s, Request::LinkFirmware, decode_link_firmware),
    )?;
    Ok(LinkSnapshot {
        info,
        state,
        firmware,
        requests: s.frames_sent() - sent_before,
        notes,
    })
}
