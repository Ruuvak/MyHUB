mod common;

use std::time::Duration;

use common::{Handle, Mock, golden, hex, pad, reply_for};
use keychron_k2he::{Event, Session, SessionError, read_snapshot};
use keychron_proto::he::{AdvanceMode, KeyMode};
use keychron_proto::{Keycode, Policy, Request, Tier};

fn session(mock: Mock) -> Session<Mock> {
    with_handle(mock).0
}

fn with_handle(mock: Mock) -> (Session<Mock>, Handle) {
    let handle = mock.handle();
    let mut s = Session::with_link(mock);
    s.set_reply_timeout(Duration::from_millis(200));
    (s, handle)
}

fn slots(region: u8) -> Request {
    Request::GetMixSlots {
        region,
        start: 0,
        count: 3,
    }
}

#[test]
fn full_snapshot_from_the_golden_mock() {
    let (mut s, h) = with_handle(Mock::golden());
    let snap = read_snapshot(&mut s).unwrap();

    assert_eq!(snap.via_protocol, 12);
    assert_eq!(snap.he.version, 4);
    assert_eq!(snap.kc.instruction_set, 2);
    assert_eq!(snap.firmware.as_deref(), Some("v1.2.1 2025-06-17-10:09:52"));
    assert!(snap.notes.is_empty(), "{:?}", snap.notes);

    assert_eq!(snap.profiles.len(), 3);
    let p0 = &snap.profiles[0].global;
    assert_eq!(p0.mode, KeyMode::Static);
    assert_eq!(p0.advance_mode, AdvanceMode::None);
    assert_eq!((p0.distance, p0.press, p0.release), (20, 4, 4));
    assert_eq!(snap.profiles[1].global.mode, KeyMode::RapidTrigger);
    assert_eq!(snap.profiles[1].name, "Gaming");

    assert_eq!(snap.lighting.led_count, Some(84));
    assert_eq!(snap.lighting.colors.as_ref().unwrap().len(), 84);
    assert_eq!(snap.lighting.mix_regions.as_ref().unwrap().len(), 84);
    assert_eq!(snap.lighting.rows.as_ref().unwrap().len(), 6);
    let slots = snap.lighting.mix_slots.as_ref().unwrap();
    assert_eq!((slots.len(), slots[0].slots.len()), (2, 5));
    assert_eq!(snap.light.brightness, Some(0xFF));
    assert_eq!(snap.curve.unwrap().points[3].y, 127);

    let keymaps = snap.keymaps.as_ref().unwrap();
    assert_eq!(keymaps.len(), 4);
    let mo3 = keymaps[2].get(5, 10).unwrap();
    assert_eq!(Keycode::decode(mo3), Keycode::Momentary(3));

    assert_eq!(snap.requests, s.frames_sent());
    // Every frame written is a Read-tier request, and the snapshot serialises.
    for report in &h.borrow().written {
        let req = Request::parse(&report[1..].try_into().unwrap()).unwrap();
        assert_eq!(req.tier(), Tier::Read);
    }
    assert!(serde_json::to_string(&snap).is_ok());
}

#[test]
fn frames_written_are_the_checked_bytes_with_report_id_zero() {
    let (mut s, h) = with_handle(Mock::golden());
    let reply = s.request(Request::GetViaProtocol).unwrap();
    assert_eq!(reply, reply_for("01"));
    let expected = Policy::read_only()
        .check(&Request::GetViaProtocol)
        .unwrap()
        .encode();
    {
        let written = &h.borrow().written;
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].len(), 33);
        assert_eq!(written[0][0], 0);
        assert_eq!(written[0][1..], expected);
    }
    // A request the policy refuses writes nothing.
    assert!(matches!(
        s.request(Request::GetLedColors { start: 0, count: 9 }),
        Err(SessionError::Policy(_))
    ));
    assert_eq!(h.borrow().written.len(), 1);
}

#[test]
fn foreign_report_fails_the_request_then_backs_off() {
    // The "launcher" answers with a command we never asked for.
    let (mut s, h) = with_handle(Mock::new(|_, _| vec![pad(&hex("a9 01 04")).to_vec()]));
    assert_eq!(
        s.request(Request::GetCurve),
        Err(SessionError::ForeignTraffic)
    );
    assert_eq!(s.foreign_count(), 1);
    assert!(matches!(
        s.request(Request::GetViaProtocol),
        Err(SessionError::BackingOff { .. })
    ));
    assert_eq!(
        h.borrow().written.len(),
        1,
        "nothing is sent while backing off"
    );

    // A foreign report already waiting stops the request before it is written.
    let (mut s, h) = with_handle(Mock::golden());
    h.borrow_mut()
        .inbox
        .push_back(pad(&hex("a8 05 00 54")).to_vec());
    assert_eq!(
        s.request(Request::GetViaProtocol),
        Err(SessionError::ForeignTraffic)
    );
    assert!(h.borrow().written.is_empty());
}

#[test]
fn unsolicited_profile_change_is_queued_and_the_request_succeeds() {
    let (mut s, h) = with_handle(Mock::golden());
    h.borrow_mut()
        .inbox
        .push_back(pad(&hex("a9 11 00 01")).to_vec());
    assert!(s.request(Request::GetViaProtocol).is_ok());
    assert_eq!(s.drain_events(), vec![Event::ProfileChanged { profile: 1 }]);
    assert!(s.drain_events().is_empty());

    // The same report arriving between the write and its reply.
    let mut s = session(Mock::new(|_, f| {
        vec![pad(&hex("a9 11 00 02")).to_vec(), golden(f).to_vec()]
    }));
    assert!(s.request(Request::GetViaProtocol).is_ok());
    assert_eq!(s.drain_events(), vec![Event::ProfileChanged { profile: 2 }]);
    assert_eq!(s.foreign_count(), 0);
}

#[test]
fn a_read_is_resent_after_a_timeout_and_gives_up_after_three_attempts() {
    // First reply lost; the second attempt gets two replies (the late one and its own).
    let (mut s, h) = with_handle(Mock::new(|n, f| match n {
        0 => vec![],
        1 => vec![golden(f).to_vec(), golden(f).to_vec()],
        _ => vec![golden(f).to_vec()],
    }));
    assert_eq!(s.request(Request::GetViaProtocol), Ok(reply_for("01")));
    assert_eq!(s.frames_sent(), 2);
    assert_eq!(h.borrow().written[0], h.borrow().written[1]);
    // The duplicate was absorbed: the next request gets its own reply.
    assert_eq!(s.request(Request::GetKcVersion), Ok(reply_for("a0")));
    assert_eq!(s.foreign_count(), 0);

    let mut s = session(Mock::new(|_, _| vec![]));
    assert_eq!(
        s.request(Request::GetViaProtocol),
        Err(SessionError::Timeout)
    );
    assert_eq!(s.frames_sent(), 3);
}

#[test]
fn a_late_duplicate_is_absorbed_and_not_taken_for_the_next_answer() {
    // Slot reads of two regions match on `a8 0e` only, so a late reply to the first
    // would pass for the second. Marker byte 3 tells them apart.
    let reply = |marker: u8| {
        let mut r = pad(&hex("a8 0e 00")).to_vec();
        r[3] = marker;
        r
    };
    let (mut s, _h) = with_handle(Mock::new(move |n, _| match n {
        0 => vec![],
        1 => vec![reply(0x11), reply(0x11)],
        _ => vec![reply(0x22)],
    }));
    assert_eq!(s.request(slots(0)).unwrap()[3], 0x11);
    assert_eq!(s.request(slots(1)).unwrap()[3], 0x22);
    assert_eq!(s.frames_sent(), 3);
    assert_eq!(s.foreign_count(), 0);
}

#[test]
fn a_reply_matching_the_pending_request_and_a_stale_entry_is_ambiguous() {
    // All three attempts of the first read time out, leaving three stale entries. The
    // next read of the same kind then gets `a8 0e ..`: ours or a late one, no way to tell.
    let (mut s, h) = with_handle(Mock::new(|n, _| match n {
        3 => vec![pad(&hex("a8 0e 00 11")).to_vec()],
        _ => vec![],
    }));
    assert_eq!(s.request(slots(0)), Err(SessionError::Timeout));
    assert_eq!(s.request(slots(1)), Err(SessionError::AmbiguousReply));
    // The other frame of the pair arrives later and is absorbed, not foreign traffic.
    h.borrow_mut()
        .inbox
        .push_back(pad(&hex("a8 0e 00 11")).to_vec());
    assert_eq!(
        s.request(Request::GetViaProtocol),
        Err(SessionError::Timeout)
    );
    assert_eq!(s.foreign_count(), 0);
    assert_eq!(h.borrow().written.len(), 7, "4 before, then 3 attempts");
    assert!(s.backoff_remaining().is_none(), "no back-off");
}

#[test]
fn poll_writes_nothing_and_reports_events_and_foreign_reports() {
    let (mut s, h) = with_handle(Mock::golden());
    h.borrow_mut()
        .inbox
        .push_back(pad(&hex("08 03 02 07")).to_vec());
    h.borrow_mut()
        .inbox
        .push_back(pad(&hex("a8 05 00 54")).to_vec());
    let wait = Duration::from_millis(1);
    assert!(matches!(
        s.poll(wait),
        Ok(Some(keychron_k2he::Incoming::Event(Event::EffectChanged {
            effect: 7
        })))
    ));
    assert!(matches!(
        s.poll(wait),
        Ok(Some(keychron_k2he::Incoming::Foreign(_)))
    ));
    assert_eq!(s.poll(wait), Ok(None));
    assert_eq!(s.foreign_count(), 1);
    assert!(h.borrow().written.is_empty());
}

#[test]
fn a_33_byte_read_with_a_leading_zero_is_stripped() {
    let mut s = session(Mock::new(|_, f| {
        let mut r = vec![0u8];
        r.extend_from_slice(&golden(f));
        vec![r]
    }));
    assert_eq!(s.request(Request::GetViaProtocol), Ok(reply_for("01")));

    // Any other length counts as foreign traffic, not guessed.
    let mut s = session(Mock::new(|_, _| vec![vec![0x01; 31]]));
    assert_eq!(
        s.request(Request::GetViaProtocol),
        Err(SessionError::BadReport(31))
    );
    assert_eq!(s.foreign_count(), 1);
    assert!(matches!(
        s.request(Request::GetViaProtocol),
        Err(SessionError::BackingOff { .. })
    ));
}

#[test]
fn an_optional_section_that_times_out_is_noted_and_skipped() {
    let mut s = session(Mock::new(|_, f| {
        if f[0] == 0xA7 && f[1] == 0x01 {
            vec![]
        } else {
            vec![golden(f).to_vec()]
        }
    }));
    let snap = read_snapshot(&mut s).unwrap();
    assert!(snap.misc.is_none());
    assert_eq!(snap.notes.len(), 1);
    assert_eq!(snap.notes[0].section, "A7 01");
    assert_eq!(snap.profiles.len(), 3, "the read went on");
}

#[test]
fn required_sections_fail_the_snapshot() {
    let mut s = session(Mock::new(|_, f| {
        if f[0] == 0xA9 && f[1] == 0x10 {
            vec![]
        } else {
            vec![golden(f).to_vec()]
        }
    }));
    assert_eq!(read_snapshot(&mut s).unwrap_err(), SessionError::Timeout);

    let mut s = session(Mock::new(|_, _| vec![pad(&hex("01 00 0b")).to_vec()]));
    assert_eq!(
        read_snapshot(&mut s).unwrap_err(),
        SessionError::UnsupportedVia(11)
    );
}
