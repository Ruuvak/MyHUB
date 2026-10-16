//! Profile select over the mock link. Nothing here opens a device.

mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use common::{Handle, Mock, hex, pad};
use keychron_k2he::{Session, SessionError, WRITE_GAP, select_profile};
use keychron_proto::{PolicyError, Request, Tier};

fn info(current: u8) -> Vec<u8> {
    let mut f = pad(&hex("a9 10 00 03 5c 03 14 14"));
    f[2] = current;
    f.to_vec()
}

/// What the keyboard is told to do.
#[derive(Clone, Copy)]
struct Behaviour {
    start: u8,
    /// A9 11 changes the profile.
    apply: bool,
    /// Byte 2 of the A9 1F reply.
    status: u8,
    /// A9 11 gets a reply.
    answer_select: bool,
    /// A9 1F gets a reply.
    answer_save: bool,
}

impl Behaviour {
    fn new(start: u8) -> Self {
        Behaviour {
            start,
            apply: true,
            status: 0,
            answer_select: true,
            answer_save: true,
        }
    }
}

fn keyboard(b: Behaviour) -> (Session<Mock>, Handle) {
    let current = Rc::new(Cell::new(b.start));
    let mock = Mock::new(move |_, f| match (f[0], f[1]) {
        (0xA9, 0x10) => vec![info(current.get())],
        (0xA9, 0x11) => {
            if b.apply {
                current.set(f[2]);
            }
            if b.answer_select {
                vec![pad(&[0xA9, 0x11, f[2]]).to_vec()]
            } else {
                vec![]
            }
        }
        (0xA9, 0x1F) if b.answer_save => vec![pad(&[0xA9, 0x1F, b.status]).to_vec()],
        (0xA9, 0x1F) => vec![],
        _ => panic!("unexpected request {:02x?}", &f[..4]),
    });
    let handle = mock.handle();
    let mut s = Session::with_link(mock);
    s.set_reply_timeout(Duration::from_millis(200));
    (s, handle)
}

/// The full 33-byte report: report ID 0, then the frame padded with zeros.
fn rep(bytes: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8];
    r.extend_from_slice(&pad(bytes));
    r
}

fn sent(h: &Handle) -> Vec<Vec<u8>> {
    h.borrow().written.iter().map(|r| r.to_vec()).collect()
}

fn count(h: &Handle, prefix: &[u8]) -> usize {
    sent(h)
        .iter()
        .filter(|r| r[1..].starts_with(prefix))
        .count()
}

#[test]
fn select_writes_a9_11_then_reads_back() {
    let (mut s, h) = keyboard(Behaviour::new(0));
    let out = select_profile(&mut s, 2, false).unwrap();
    assert_eq!(
        sent(&h),
        [
            rep(&[0xA9, 0x10]),
            rep(&[0xA9, 0x11, 2]),
            rep(&[0xA9, 0x10])
        ]
    );
    assert_eq!((out.before, out.after), (0, 2));
    assert!(!out.saved && !out.unchanged);
    assert_eq!(out.frames_written, 1);
}

#[test]
fn unchanged_profile_without_save_writes_nothing() {
    let (mut s, h) = keyboard(Behaviour::new(1));
    let out = select_profile(&mut s, 1, false).unwrap();
    assert_eq!(sent(&h), [rep(&[0xA9, 0x10])]);
    assert!(out.unchanged && !out.saved);
    assert_eq!((out.before, out.after, out.frames_written), (1, 1, 0));
}

#[test]
fn a_profile_that_did_not_stick_fails_verification() {
    let (mut s, h) = keyboard(Behaviour {
        apply: false,
        ..Behaviour::new(0)
    });
    let err = select_profile(&mut s, 2, false).unwrap_err();
    assert_eq!(
        err,
        SessionError::VerifyFailed {
            expected: 2,
            got: 0,
            saved: false
        }
    );
    assert_eq!(sent(&h).len(), 3);
}

#[test]
fn save_sends_a9_1f_only_when_asked_and_checks_the_status() {
    let (mut s, h) = keyboard(Behaviour::new(0));
    let out = select_profile(&mut s, 1, true).unwrap();
    assert_eq!(
        sent(&h),
        [
            rep(&[0xA9, 0x10]),
            rep(&[0xA9, 0x11, 1]),
            rep(&[0xA9, 0x10]),
            rep(&[0xA9, 0x10]),
            rep(&[0xA9, 0x1F, 1]),
            rep(&[0xA9, 0x10])
        ]
    );
    assert!(out.saved);
    assert_eq!((out.before, out.after, out.frames_written), (0, 1, 2));

    let (mut s, h) = keyboard(Behaviour {
        status: 3,
        ..Behaviour::new(0)
    });
    let err = select_profile(&mut s, 1, true).unwrap_err();
    assert_eq!(
        err,
        SessionError::DeviceStatus {
            command: 0xA9,
            sub: 0x1F,
            status: 3
        }
    );
    assert_eq!(sent(&h).last().unwrap(), &rep(&[0xA9, 0x1F, 1]));
}

#[test]
fn same_profile_with_save_sends_only_a9_1f() {
    let (mut s, h) = keyboard(Behaviour::new(1));
    let out = select_profile(&mut s, 1, true).unwrap();
    assert_eq!(
        sent(&h),
        [
            rep(&[0xA9, 0x10]),
            rep(&[0xA9, 0x10]),
            rep(&[0xA9, 0x1F, 1]),
            rep(&[0xA9, 0x10])
        ]
    );
    assert_eq!(count(&h, &[0xA9, 0x11]), 0);
    assert!(out.saved && !out.unchanged);
    assert_eq!(out.frames_written, 1);
}

#[test]
fn no_save_when_the_current_profile_is_not_the_one_asked_for() {
    // A9 11 is acknowledged but does not stick: the verify after it fails first, so
    // A9 1F is never sent.
    let (mut s, h) = keyboard(Behaviour {
        apply: false,
        ..Behaviour::new(0)
    });
    let err = select_profile(&mut s, 2, true).unwrap_err();
    assert!(matches!(
        err,
        SessionError::VerifyFailed { saved: false, .. }
    ));
    assert_eq!(count(&h, &[0xA9, 0x1F]), 0);
}

#[test]
fn writes_are_spaced_by_the_write_gap() {
    let (mut s, _h) = keyboard(Behaviour::new(0));
    let start = std::time::Instant::now();
    select_profile(&mut s, 1, true).unwrap();
    assert!(start.elapsed() >= WRITE_GAP);
}

#[test]
fn a_write_timeout_is_not_retried() {
    let (mut s, h) = keyboard(Behaviour {
        answer_select: false,
        ..Behaviour::new(0)
    });
    assert_eq!(
        select_profile(&mut s, 2, false).unwrap_err(),
        SessionError::Timeout
    );
    assert_eq!(count(&h, &[0xA9, 0x11]), 1);
}

#[test]
fn an_a9_1f_timeout_is_not_retried() {
    let (mut s, h) = keyboard(Behaviour {
        answer_save: false,
        ..Behaviour::new(1)
    });
    assert_eq!(
        select_profile(&mut s, 1, true).unwrap_err(),
        SessionError::Timeout
    );
    assert_eq!(count(&h, &[0xA9, 0x1F]), 1);
}

#[test]
fn foreign_traffic_before_a_write_sends_no_write() {
    let (mut s, h) = keyboard(Behaviour::new(0));
    h.borrow_mut()
        .inbox
        .push_back(pad(&[0xA8, 0x06, 1]).to_vec());
    assert_eq!(
        select_profile(&mut s, 2, true).unwrap_err(),
        SessionError::ForeignTraffic
    );
    assert_eq!(count(&h, &[0xA9, 0x11]), 0);
    assert_eq!(count(&h, &[0xA9, 0x1F]), 0);
}

#[test]
fn the_session_policy_stays_read_only_after_a_write() {
    let (mut s, _h) = keyboard(Behaviour::new(0));
    select_profile(&mut s, 1, true).unwrap();
    assert_eq!(
        s.request(Request::SelectProfile { profile: 2 }),
        Err(SessionError::Policy(PolicyError::TierNotEnabled {
            tier: Tier::Volatile
        }))
    );
    assert_eq!(
        s.request(Request::SaveProfile { profile: 2 }),
        Err(SessionError::Policy(PolicyError::TierNotEnabled {
            tier: Tier::Persistent
        }))
    );
}

#[test]
fn a_profile_past_the_count_is_refused_before_any_write() {
    let (mut s, h) = keyboard(Behaviour::new(0));
    let err = select_profile(&mut s, 3, false).unwrap_err();
    assert!(matches!(
        err,
        SessionError::Policy(PolicyError::ProfileOutOfRange { .. })
    ));
    assert_eq!(sent(&h), [rep(&[0xA9, 0x10])]);
}

#[test]
fn a_profile_changed_on_the_keyboard_before_the_save_refuses_it() {
    // The first A9 10 says profile 1, the pre-save read says 0 (hotkey pressed).
    let reads = Rc::new(Cell::new(0u8));
    let mock = Mock::new(move |_, f| {
        assert_eq!((f[0], f[1]), (0xA9, 0x10), "only reads may be sent");
        reads.set(reads.get() + 1);
        vec![info(if reads.get() == 1 { 1 } else { 0 })]
    });
    let h = mock.handle();
    let mut s = Session::with_link(mock);
    let err = select_profile(&mut s, 1, true).unwrap_err();
    assert_eq!(
        err,
        SessionError::VerifyFailed {
            expected: 1,
            got: 0,
            saved: false
        }
    );
    assert_eq!(count(&h, &[0xA9, 0x1F]), 0);
}
