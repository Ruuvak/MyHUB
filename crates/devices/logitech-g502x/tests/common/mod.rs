//! Mock HID link driven by the real device frame log. Nothing here opens a
//! device or touches hidapi.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use myhub_transport::{HidLink, HidppSession, LinkError};

/// Real device frames (`TX`/`RX` lines), owned by the hidpp crate.
pub const FRAMES_LOG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../hidpp/tests/fixtures/frames.log"
));
/// Probe output of the real device.
pub const FIXTURE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../hidpp/tests/fixtures/hidpp_read.json"
));

/// Feature index of 0x8100 in the fixture table.
pub const IDX_ONBOARD: u8 = 9;
/// Feature index of 0x1004 in the fixture table.
pub const IDX_BATTERY: u8 = 6;

/// Synthetic 8100 fn2 (IsOnboardMode) exchange. Not in the log: the device was
/// never asked via the log; the live read in the BRIEF [OBS] line gave `01`,
/// which is ONBOARD mode on the wire (0x02 is host).
pub const SYNTH_FN2_TX: &str = "1101092b00000000000000000000000000000000";
pub const SYNTH_FN2_RX: &str = "1101092b01000000000000000000000000000000";
/// The same exchange answered in host mode.
pub const SYNTH_FN2_HOST_RX: &str = "1101092b02000000000000000000000000000000";
/// Synthetic 8100 fn4 (GetActiveProfile) exchange, invented test data.
pub const SYNTH_FN4_TX: &str = "1101094b00000000000000000000000000000000";
pub const SYNTH_FN4_RX: &str = "1101094b00010000000000000000000000000000";

pub fn hex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
        .collect()
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A 20 byte long report from a prefix, zero padded.
pub fn long(prefix: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    v.resize(20, 0);
    v
}

/// `(tx, rx)` pairs from the frame log, in log order.
pub fn logged_pairs() -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut pairs = Vec::new();
    let mut pending: Option<Vec<u8>> = None;
    for line in FRAMES_LOG.lines().filter(|l| !l.trim().is_empty()) {
        let (tag, rest) = line.split_once(' ').expect("log line");
        match tag {
            "TX" => {
                assert!(pending.is_none(), "TX without RX");
                pending = Some(hex(rest.trim()));
            }
            "RX" => pairs.push((pending.take().expect("RX without TX"), hex(rest.trim()))),
            other => panic!("unexpected log tag {other}"),
        }
    }
    assert!(pending.is_none(), "dangling TX");
    pairs
}

#[derive(Default)]
pub struct MockState {
    /// Reply frames per TX frame.
    pub responses: HashMap<Vec<u8>, Vec<Vec<u8>>>,
    /// Frames waiting to be read.
    pub inbox: VecDeque<Vec<u8>>,
    /// Every frame the session wrote, in order.
    pub writes: Vec<Vec<u8>>,
    /// Delivered before the replies of the next TX.
    pub pre_reply: Vec<Vec<u8>>,
    /// Replaces the replies of the next TX (an empty list means silence).
    pub override_reply: Option<Vec<Vec<u8>>>,
    /// What the link reports for `short_reports_available`.
    pub short_available: bool,
}

impl MockState {
    pub fn set_reply(&mut self, tx: Vec<u8>, frames: Vec<Vec<u8>>) {
        self.responses.insert(tx, frames);
    }
}

pub type Handle = Rc<RefCell<MockState>>;

/// The link handed to the session. The test keeps the [`Handle`].
pub struct MockLink(Handle);

impl HidLink for MockLink {
    fn write_report(&mut self, report: &[u8; 20]) -> Result<(), LinkError> {
        let mut state = self.0.borrow_mut();
        state.writes.push(report.to_vec());
        let replies = match state.override_reply.take() {
            Some(frames) => frames,
            None => state
                .responses
                .get(report.as_slice())
                .unwrap_or_else(|| panic!("unexpected TX with no reply: {}", to_hex(report)))
                .clone(),
        };
        let pre = std::mem::take(&mut state.pre_reply);
        state.inbox.extend(pre);
        state.inbox.extend(replies);
        Ok(())
    }

    fn short_reports_available(&self) -> bool {
        self.0.borrow().short_available
    }

    fn read_report(&mut self, buf: &mut [u8; 64], _timeout_ms: i32) -> Result<usize, LinkError> {
        let next = self.0.borrow_mut().inbox.pop_front();
        match next {
            Some(frame) => {
                buf[..frame.len()].copy_from_slice(&frame);
                Ok(frame.len())
            }
            None => {
                // Stand-in for a blocking read that times out.
                std::thread::sleep(Duration::from_millis(1));
                Ok(0)
            }
        }
    }
}

/// Mock loaded with every logged exchange plus the two synthetic 8100 reads.
pub fn fixture_link() -> (MockLink, Handle) {
    let handle: Handle = Rc::new(RefCell::new(MockState::default()));
    {
        let mut state = handle.borrow_mut();
        state.short_available = true;
        for (tx, rx) in logged_pairs() {
            let previous = state.responses.insert(tx, vec![rx.clone()]);
            assert!(
                previous.is_none() || previous == Some(vec![rx]),
                "log has one TX with two different replies"
            );
        }
        state.set_reply(hex(SYNTH_FN2_TX), vec![hex(SYNTH_FN2_RX)]);
        state.set_reply(hex(SYNTH_FN4_TX), vec![hex(SYNTH_FN4_RX)]);
    }
    (MockLink(handle.clone()), handle)
}

/// A session over the fixture mock, discovery not yet run.
pub fn fixture_session() -> (HidppSession<MockLink>, Handle) {
    let (link, handle) = fixture_link();
    let mut session = logitech_g502x::new_session(link);
    // Mock reads return at once, so a silent device would otherwise spin for
    // the full 1.5 s default.
    session.set_reply_timeout(Duration::from_millis(100));
    (session, handle)
}

/// A session over the fixture mock with discovery done.
pub fn discovered_session() -> (HidppSession<MockLink>, Handle) {
    let (mut session, handle) = fixture_session();
    session.discover().expect("discover over the fixture");
    (session, handle)
}
