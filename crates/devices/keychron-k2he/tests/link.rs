//! Receiver session over a mock link. Nothing here opens a device.

mod common;

use std::time::Duration;

use common::{Handle, Mock, pad};
use keychron_k2he::{Session, SessionError, read_link, read_link_state};
use keychron_proto::{Request, Target};

const B1: [u8; 4] = [0xB1, 0x01, 0x00, 0x08];
const B2_UP: [u8; 7] = [0xB2, 0x01, 0x34, 0x34, 0x20, 0x0E, 0x01];
const B2_DOWN: [u8; 7] = [0xB2, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00];

fn b3() -> Vec<u8> {
    let mut v = vec![0xB3];
    v.extend_from_slice(b"d.3.1 Jan 5 2026 16:57:08");
    v
}

fn receiver(mock: Mock) -> (Session<Mock>, Handle) {
    let handle = mock.handle();
    let mut s = Session::with_link_for(mock, Target::Receiver);
    s.set_reply_timeout(Duration::from_millis(200));
    (s, handle)
}

/// Answers B1, B2 and B3 like the real receiver, B2 with `state`.
fn receiver_mock(state: &'static [u8]) -> Mock {
    Mock::new(move |_, f| match f[0] {
        0xB1 => vec![pad(&B1).to_vec()],
        0xB2 => vec![pad(state).to_vec()],
        0xB3 => vec![pad(&b3()).to_vec()],
        _ => vec![],
    })
}

#[test]
fn state_decodes_connected_and_disconnected() {
    let (mut s, _) = receiver(receiver_mock(&B2_UP));
    let up = read_link_state(&mut s).unwrap();
    assert!(up.connected && up.is_k2he());
    assert_eq!((up.linked_vid, up.linked_pid), (0x3434, 0x0E20));

    let (mut s, _) = receiver(receiver_mock(&B2_DOWN));
    let down = read_link_state(&mut s).unwrap();
    assert!(!down.connected && !down.is_k2he());
}

#[test]
fn keyboard_request_on_a_receiver_session_is_refused_before_any_byte() {
    let (mut s, h) = receiver(receiver_mock(&B2_UP));
    assert_eq!(
        s.request(Request::GetViaProtocol),
        Err(SessionError::TargetMismatch {
            request: Target::Keyboard,
            session: Target::Receiver
        })
    );
    assert!(h.borrow().written.is_empty());
    assert_eq!(s.frames_sent(), 0);
}

#[test]
fn select_profile_on_a_receiver_session_writes_nothing() {
    let (mut s, h) = receiver(receiver_mock(&B2_UP));
    assert!(matches!(
        keychron_k2he::select_profile(&mut s, 1, false),
        Err(SessionError::TargetMismatch {
            request: Target::Keyboard,
            session: Target::Receiver
        })
    ));
    assert!(h.borrow().written.is_empty());
}

#[test]
fn receiver_request_on_a_keyboard_session_is_refused_before_any_byte() {
    let mock = Mock::golden();
    let h = mock.handle();
    let mut s = Session::with_link(mock);
    assert_eq!(s.target(), Target::Keyboard);
    assert_eq!(
        read_link_state(&mut s),
        Err(SessionError::TargetMismatch {
            request: Target::Receiver,
            session: Target::Keyboard
        })
    );
    assert!(h.borrow().written.is_empty());
}

#[test]
fn read_link_sends_b1_b2_b3_once_each_in_order() {
    let (mut s, h) = receiver(receiver_mock(&B2_UP));
    let snap = read_link(&mut s).unwrap();

    let info = snap.info.unwrap();
    assert_eq!(info.version, 1);
    assert!(info.flags.via_disable_gamepad_input);
    assert!(snap.state.is_k2he());
    assert_eq!(snap.firmware.as_deref(), Some("d.3.1 Jan 5 2026 16:57:08"));
    assert_eq!(snap.requests, 3);
    assert!(snap.notes.is_empty(), "{:?}", snap.notes);
    assert!(serde_json::to_string(&snap).is_ok());

    let written = &h.borrow().written;
    assert_eq!(written.len(), 3);
    for (report, command) in written.iter().zip([0xB1u8, 0xB2, 0xB3]) {
        assert_eq!(report[0], 0, "report ID");
        assert_eq!(report[1], command);
        assert!(report[2..].iter().all(|&b| b == 0));
    }
}

#[test]
fn b2_timeout_fails_after_the_read_retries() {
    let mock = Mock::new(|_, f| match f[0] {
        0xB1 => vec![pad(&B1).to_vec()],
        _ => vec![],
    });
    let (mut s, h) = receiver(mock);
    assert_eq!(read_link(&mut s), Err(SessionError::Timeout));
    // B1 once, B2 three times (one try and two resends), no B3.
    let commands: Vec<u8> = h.borrow().written.iter().map(|r| r[1]).collect();
    assert_eq!(commands, [0xB1, 0xB2, 0xB2, 0xB2]);
}

#[test]
fn b1_and_b3_timeouts_are_notes_not_failures() {
    let mock = Mock::new(|_, f| match f[0] {
        0xB2 => vec![pad(&B2_UP).to_vec()],
        _ => vec![],
    });
    let (mut s, _) = receiver(mock);
    let snap = read_link(&mut s).unwrap();
    assert!(snap.info.is_none() && snap.firmware.is_none());
    assert!(snap.state.is_k2he());
    assert_eq!(snap.notes.len(), 2);
}
