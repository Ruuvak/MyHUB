//! spy_capture against a mock link: start, events, stop always. No device.

mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use common::{
    Handle, IDX_ONBOARD, MockLink, SYNTH_FN2_HOST_RX, SYNTH_FN2_RX, SYNTH_FN2_TX,
    discovered_session, hex, long,
};
use hidpp::FeatureId;
use hidpp::policy::PolicyError;
use logitech_g502x::{SpyError, SpyLine, describe_spy_payload, spy_capture};
use myhub_transport::{HidLink, HidppSession, LinkError, SessionError};

/// Feature index of 0x8110 in the fixture table.
const IDX_SPY: u8 = 10;
/// fn1 / fn2 requests with the default software ID 0x0B.
const START_TX: &str = "11010a1b00000000000000000000000000000000";
const STOP_TX: &str = "11010a2b00000000000000000000000000000000";

fn host_session() -> (HidppSession<MockLink>, Handle) {
    let (session, handle) = discovered_session();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(hex(SYNTH_FN2_TX), vec![hex(SYNTH_FN2_HOST_RX)]);
        state.set_reply(
            hex(START_TX),
            vec![hex("11010a1b00000000000000000000000000000000")],
        );
        state.set_reply(
            hex(STOP_TX),
            vec![hex("11010a2b00000000000000000000000000000000")],
        );
    }
    (session, handle)
}

/// Every frame the session wrote for 0x8110, as (function, params).
fn spy_frames(handle: &Handle) -> Vec<(u8, Vec<u8>)> {
    handle
        .borrow()
        .writes
        .iter()
        .filter(|f| f[2] == IDX_SPY)
        .map(|f| (f[3] >> 4, f[4..].to_vec()))
        .collect()
}

fn event(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x11, 0x01, IDX_SPY, 0x00];
    frame.extend_from_slice(payload);
    frame.resize(20, 0);
    frame
}

fn run(
    session: &mut HidppSession<impl HidLink>,
    ms: u64,
) -> (Result<logitech_g502x::SpyReport, SpyError>, Vec<SpyLine>) {
    let mut lines = Vec::new();
    let result = spy_capture(session, Duration::from_millis(ms), &|| false, &mut |l| {
        lines.push(l)
    });
    (result, lines)
}

#[test]
fn payload_description_shows_raw_be_le_and_bits() {
    let mut p = [0u8; 16];
    p[0] = 0x00;
    p[1] = 0x05;
    let text = describe_spy_payload(0, &p);
    assert!(text.contains("raw=00050000"), "{text}");
    assert!(
        text.contains("BE=0x0005") && text.contains("bitsBE=[0,2]"),
        "{text}"
    );
    assert!(
        text.contains("LE=0x0500") && text.contains("bitsLE=[8,10]"),
        "{text}"
    );
}

#[test]
fn start_events_stop_in_order_and_nothing_else_is_sent() {
    let (mut session, handle) = host_session();
    // Two events arrive while listening, one other notification too.
    {
        let mut state = handle.borrow_mut();
        state.pre_reply = vec![event(&[0x00, 0x04])];
        state.inbox.push_back(event(&[0x00, 0x00]));
        state
            .inbox
            .push_back(long(&[0x11, 0x01, 0x04, 0x00, 0x01, 0x01, 0x01]));
    }
    let (result, lines) = run(&mut session, 250);
    let report = result.expect("capture");
    assert_eq!(report.spy_events, 2);
    assert_eq!(report.other_events, 1);
    assert_eq!(report.stop, Ok(()));
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert!(texts[0].starts_with("spy started"));
    assert!(
        texts
            .iter()
            .any(|t| t.contains("raw=00040000") && t.contains("BE=0x0004"))
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("other index 4 fn0 raw=010101"))
    );
    assert!(texts.last().unwrap().starts_with("spy stopped"));

    // Frames: fn2 mode read (0x8100), start, stop. Only 0x8110 fn1/fn2 with
    // zero parameters were sent for the spy, start before stop.
    let frames = spy_frames(&handle);
    assert_eq!(frames, vec![(1, vec![0; 16]), (2, vec![0; 16])]);
    let writes = handle.borrow().writes.clone();
    let tail: Vec<(u8, u8)> = writes
        .iter()
        .filter(|f| f[2] == IDX_SPY || f[2] == IDX_ONBOARD)
        .map(|f| (f[2], f[3] >> 4))
        .collect();
    assert_eq!(tail, vec![(IDX_ONBOARD, 2), (IDX_SPY, 1), (IDX_SPY, 2)]);
    // Nothing but those frames after discovery.
    assert!(
        !writes
            .iter()
            .any(|f| f[2] == IDX_SPY && matches!(f[3] >> 4, 3 | 4))
    );
}

#[test]
fn onboard_mode_is_refused_before_anything_is_started() {
    let (mut session, handle) = host_session();
    handle
        .borrow_mut()
        .set_reply(hex(SYNTH_FN2_TX), vec![hex(SYNTH_FN2_RX)]);
    let (result, lines) = run(&mut session, 100);
    assert_eq!(result.unwrap_err(), SpyError::NotHostMode);
    assert!(lines.is_empty());
    assert!(spy_frames(&handle).is_empty());
}

#[test]
fn a_stop_request_ends_the_capture_and_still_sends_stop() {
    let (mut session, handle) = host_session();
    let polls = Cell::new(0);
    let report = spy_capture(
        &mut session,
        Duration::from_secs(30),
        &|| {
            polls.set(polls.get() + 1);
            polls.get() > 1
        },
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(report.stop, Ok(()));
    assert_eq!(spy_frames(&handle).len(), 2);
}

/// A link whose reads fail once `broken` is set (a USB read error).
struct FlakyLink {
    inner: MockLink,
    broken: Rc<Cell<bool>>,
}

impl HidLink for FlakyLink {
    fn write_report(&mut self, report: &[u8; 20]) -> Result<(), LinkError> {
        self.inner.write_report(report)
    }
    fn short_reports_available(&self) -> bool {
        self.inner.short_reports_available()
    }
    fn read_report(&mut self, buf: &mut [u8; 64], timeout_ms: i32) -> Result<usize, LinkError> {
        if self.broken.get() {
            return Err(LinkError::Hid("read failed".into()));
        }
        self.inner.read_report(buf, timeout_ms)
    }
}

#[test]
fn a_read_error_after_the_start_makes_the_stop_fail_visibly() {
    let (link, handle) = common::fixture_link();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(hex(SYNTH_FN2_TX), vec![hex(SYNTH_FN2_HOST_RX)]);
        state.set_reply(hex(START_TX), vec![hex(START_TX)]);
        state.set_reply(hex(STOP_TX), vec![hex(STOP_TX)]);
    }
    let broken = Rc::new(Cell::new(false));
    let mut session = logitech_g502x::new_session(FlakyLink {
        inner: link,
        broken: broken.clone(),
    });
    session.set_reply_timeout(Duration::from_millis(100));
    session.discover().unwrap();

    // The link breaks once the capture is listening: the first quiet poll fails.
    let flag = broken.clone();
    let mut seen = 0;
    let result = spy_capture(
        &mut session,
        Duration::from_secs(5),
        &|| false,
        &mut |line| {
            seen += 1;
            if line.text.starts_with("spy started") {
                flag.set(true);
            }
        },
    );
    let SpyError::Failed { cause, stop } = result.unwrap_err() else {
        panic!("expected Failed");
    };
    assert!(matches!(cause, SessionError::Link(_)), "{cause:?}");
    // A link whose reads fail cannot take the stop either: the session drains
    // the link before every write and that read fails first, so no byte goes out
    // and the failure is reported (the stop is retried once, with the same result).
    let frames = spy_frames(&handle);
    assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), vec![1]);
    assert!(matches!(stop, Err(SessionError::Link(_))), "{stop:?}");
    assert!(seen >= 2);
}

#[test]
fn a_refused_or_failed_start_still_attempts_the_stop() {
    let (mut session, handle) = host_session();
    // The mouse errors on the start frame (HID++ 2.0 error 2 for feature 10).
    handle.borrow_mut().set_reply(
        hex(START_TX),
        vec![long(&[0x11, 0x01, 0xFF, IDX_SPY, 0x1B, 0x02])],
    );
    let (result, _) = run(&mut session, 100);
    let SpyError::Failed { cause, stop } = result.unwrap_err() else {
        panic!("expected Failed");
    };
    assert!(
        matches!(cause, SessionError::Device { code: 2, .. }),
        "{cause:?}"
    );
    assert_eq!(stop, Ok(()));
    let frames = spy_frames(&handle);
    assert_eq!(frames.iter().map(|f| f.0).collect::<Vec<_>>(), vec![1, 2]);
}

/// A link that queues `after_start` frames once the spy start frame was written
/// (so they arrive while the capture listens, not before the start).
struct InjectLink {
    inner: MockLink,
    handle: Handle,
    after_start: Vec<Vec<u8>>,
}

impl HidLink for InjectLink {
    fn write_report(&mut self, report: &[u8; 20]) -> Result<(), LinkError> {
        self.inner.write_report(report)?;
        if report[2] == IDX_SPY && report[3] >> 4 == 1 {
            let frames = std::mem::take(&mut self.after_start);
            self.handle.borrow_mut().inbox.extend(frames);
        }
        Ok(())
    }
    fn short_reports_available(&self) -> bool {
        self.inner.short_reports_available()
    }
    fn read_report(&mut self, buf: &mut [u8; 64], timeout_ms: i32) -> Result<usize, LinkError> {
        self.inner.read_report(buf, timeout_ms)
    }
}

#[test]
fn a_lost_link_is_reported_and_the_stop_is_refused_before_any_byte() {
    let (link, handle) = common::fixture_link();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(hex(SYNTH_FN2_TX), vec![hex(SYNTH_FN2_HOST_RX)]);
        state.set_reply(hex(START_TX), vec![hex(START_TX)]);
        state.set_reply(hex(STOP_TX), vec![hex(STOP_TX)]);
    }
    // A 0x41 "link down" for device 1 arrives after the start, while listening.
    let mut session = logitech_g502x::new_session(InjectLink {
        inner: link,
        handle: handle.clone(),
        after_start: vec![long(&[0x11, 0x01, 0x41, 0x00, 0x40, 0x9f, 0x40])],
    });
    session.set_reply_timeout(Duration::from_millis(100));
    session.discover().unwrap();
    let (result, lines) = run(&mut session, 2000);
    let SpyError::Failed { cause, stop } = result.unwrap_err() else {
        panic!("expected Failed");
    };
    assert_eq!(cause, SessionError::NeedsRediscovery);
    assert_eq!(stop, Err(SessionError::NeedsRediscovery));
    assert!(lines.iter().any(|l| l.text.contains("link down")));
    // Only the start frame went out: the stop could not be sent.
    assert_eq!(
        spy_frames(&handle).iter().map(|f| f.0).collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn only_the_spy_entry_point_reaches_0x8110() {
    let (mut session, handle) = host_session();
    let before = handle.borrow().writes.len();
    let spy = FeatureId::MOUSE_BUTTON_SPY;
    for function in 0..=15u8 {
        for err in [
            session.request_feature(spy, function, [0; 16]).unwrap_err(),
            session
                .request_volatile(spy, function, [0; 16])
                .unwrap_err(),
        ] {
            assert!(
                matches!(
                    err,
                    SessionError::Policy(PolicyError::NotAllowlisted { .. })
                ),
                "fn {function}: {err:?}"
            );
        }
    }
    assert_eq!(handle.borrow().writes.len(), before, "a frame was sent");
}

#[test]
fn spy_stop_sends_exactly_one_fn2_frame_in_any_mode() {
    use logitech_g502x::spy_stop;
    // Default fixture mode is onboard: spy-stop has no mode requirement.
    let (mut session, handle) = discovered_session();
    handle.borrow_mut().set_reply(
        hex(STOP_TX),
        vec![hex("11010a2b00000000000000000000000000000000")],
    );
    let before = handle.borrow().writes.len();
    let reply = spy_stop(&mut session).unwrap();
    assert_eq!(reply, [0u8; 16]);
    let writes = handle.borrow().writes.clone();
    assert_eq!(writes.len(), before + 1, "more than one frame was sent");
    assert_eq!(writes.last().unwrap().as_slice(), hex(STOP_TX).as_slice());
    assert_eq!(spy_frames(&handle), vec![(2, vec![0; 16])]);
}
