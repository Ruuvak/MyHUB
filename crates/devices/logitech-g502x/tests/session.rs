//! `HidppSession` behaviour over the mock link. No hardware, no hidapi.

mod common;

use std::time::Duration;

use common::*;
use hidpp::{FeatureId, Message, PolicyError};
use myhub_transport::{ErrorFamily, NOTIFICATION_CAPACITY, SessionError};

const NO_PARAMS: [u8; 16] = [0; 16];

fn battery_tx() -> Vec<u8> {
    long(&[0x11, 0x01, IDX_BATTERY, 0x1b])
}

fn battery_reply() -> Vec<u8> {
    long(&[0x11, 0x01, IDX_BATTERY, 0x1b, 0x37, 0x04])
}

#[test]
fn discovery_fills_the_table() {
    let (session, _handle) = discovered_session();
    assert_eq!(session.table().len(), 28);
    assert!(!session.needs_rediscovery());
    assert_eq!(
        session.table().index_of(FeatureId::UNIFIED_BATTERY),
        Some(IDX_BATTERY)
    );
    assert_eq!(
        session.table().index_of(FeatureId::ONBOARD_PROFILES),
        Some(IDX_ONBOARD)
    );
    assert_eq!(session.request_count(), 30);
}

#[test]
fn requests_are_refused_before_discovery() {
    let (mut session, handle) = fixture_session();
    let err = session.request(IDX_BATTERY, 1, NO_PARAMS).unwrap_err();
    assert_eq!(err, SessionError::NeedsRediscovery);
    let err = session
        .request_feature(FeatureId::UNIFIED_BATTERY, 1, NO_PARAMS)
        .unwrap_err();
    assert_eq!(err, SessionError::NeedsRediscovery);
    assert!(handle.borrow().writes.is_empty());
}

#[test]
fn foreign_replies_and_notifications_are_skipped() {
    let (mut session, handle) = discovered_session();
    handle.borrow_mut().pre_reply = vec![
        // Same request answered to another software ID (G HUB, kernel).
        long(&[0x11, 0x01, IDX_BATTERY, 0x1c, 0x01, 0x01]),
        long(&[0x11, 0x01, IDX_BATTERY, 0x11, 0x02, 0x02]),
        // Another device behind the receiver, same indexes and software ID.
        long(&[0x11, 0x02, IDX_BATTERY, 0x1b, 0x03, 0x03]),
        // Unrelated feature index with our software ID.
        long(&[0x11, 0x01, IDX_ONBOARD, 0x1b, 0x04, 0x04]),
        // HID++ 2.0 notification (software ID 0).
        long(&[0x11, 0x01, IDX_BATTERY, 0x00, 0x5a, 0x01]),
        // HID++ 1.0 notification other than 0x41, and a 0x8F error for a
        // request we did not send.
        hex("10014b01000000"),
        hex("10018f09110100"),
        // Not a HID++ report at all.
        long(&[0x20, 0x01, 0x02]),
    ];
    let payload = session
        .request_feature(FeatureId::UNIFIED_BATTERY, 1, NO_PARAMS)
        .expect("reply");
    assert_eq!(payload[0], 0x37);

    let notes = session.drain_notifications();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(matches!(
        notes[0],
        Message::Notification { function: 0, .. }
    ));
    assert!(matches!(notes[1], Message::Hidpp10 { sub_id: 0x4b, .. }));
    assert!(session.drain_notifications().is_empty());
    assert!(!session.needs_rediscovery());
}

#[test]
fn notification_buffer_is_bounded_and_drops_the_oldest() {
    let (mut session, handle) = discovered_session();
    let extra = 6;
    handle.borrow_mut().pre_reply = (0..NOTIFICATION_CAPACITY + extra)
        .map(|i| long(&[0x11, 0x01, IDX_BATTERY, 0x00, i as u8]))
        .collect();
    session
        .request_feature(FeatureId::UNIFIED_BATTERY, 1, NO_PARAMS)
        .expect("reply");
    let notes = session.drain_notifications();
    assert_eq!(notes.len(), NOTIFICATION_CAPACITY);
    assert_eq!(session.dropped_notifications(), extra as u64);
    // The oldest `extra` entries (payload byte 0 = 0..extra) are gone.
    let Message::Notification { payload, .. } = &notes[0] else {
        panic!("not a notification");
    };
    assert_eq!(usize::from(payload[0]), extra);
}

#[test]
fn hidpp20_error_frame_becomes_a_device_error() {
    let (mut session, handle) = discovered_session();
    handle.borrow_mut().override_reply =
        Some(vec![long(&[0x11, 0x01, 0xFF, IDX_BATTERY, 0x1b, 0x02])]);
    let err = session.request(IDX_BATTERY, 1, NO_PARAMS).unwrap_err();
    assert_eq!(
        err,
        SessionError::Device {
            code: 2,
            family: ErrorFamily::Hidpp20,
            name: Some("InvalidArgument"),
        }
    );
    assert!(err.to_string().contains("InvalidArgument"));
}

#[test]
fn hidpp10_error_becomes_a_device_error() {
    let (mut session, handle) = discovered_session();
    // 10 dev 8F featureIndex fn|sw errorCode 00
    handle.borrow_mut().override_reply = Some(vec![hex("10018f061b0100")]);
    let err = session.request(IDX_BATTERY, 1, NO_PARAMS).unwrap_err();
    assert_eq!(
        err,
        SessionError::Device {
            code: 1,
            family: ErrorFamily::Hidpp10,
            name: None,
        }
    );
}

#[test]
fn silence_gives_a_timeout() {
    let (mut session, handle) = discovered_session();
    session.set_reply_timeout(Duration::from_millis(30));
    handle.borrow_mut().override_reply = Some(vec![]);
    let err = session.request(IDX_BATTERY, 1, NO_PARAMS).unwrap_err();
    assert_eq!(err, SessionError::Timeout);
    // A timeout is not a table problem.
    assert!(!session.needs_rediscovery());
}

#[test]
fn connection_notification_clears_the_table_until_rediscovery() {
    let (mut session, handle) = discovered_session();
    // HID++ 1.0 0x41 device connection, then the real reply.
    handle.borrow_mut().pre_reply = vec![hex("10014104000000")];
    let err = session.request(IDX_BATTERY, 1, NO_PARAMS).unwrap_err();
    assert_eq!(err, SessionError::NeedsRediscovery);
    assert!(session.table().is_empty());
    assert!(session.needs_rediscovery());
    // The notification itself is kept for the application.
    let notes = session.drain_notifications();
    assert!(matches!(notes[0], Message::Hidpp10 { sub_id: 0x41, .. }));

    // Every request is refused without a write until discover() ran.
    let writes_before = handle.borrow().writes.len();
    assert_eq!(
        session.request(IDX_BATTERY, 1, NO_PARAMS).unwrap_err(),
        SessionError::NeedsRediscovery
    );
    assert_eq!(
        session
            .request_feature(FeatureId::UNIFIED_BATTERY, 1, NO_PARAMS)
            .unwrap_err(),
        SessionError::NeedsRediscovery
    );
    assert_eq!(handle.borrow().writes.len(), writes_before);

    // Rediscovery restores service. (The stale reply of the aborted request is
    // still in the mock inbox and must be ignored.)
    session.discover().expect("rediscover");
    assert_eq!(session.table().len(), 28);
    assert!(!session.needs_rediscovery());
    let payload = session
        .request_feature(FeatureId::UNIFIED_BATTERY, 1, NO_PARAMS)
        .expect("reply after rediscovery");
    assert_eq!(payload[0], 0x37);
}

#[test]
fn failed_discovery_leaves_the_session_blocked() {
    let (mut session, handle) = fixture_session();
    session.set_reply_timeout(Duration::from_millis(30));
    handle.borrow_mut().override_reply = Some(vec![]);
    assert_eq!(session.discover().unwrap_err(), SessionError::Timeout);
    assert!(session.table().is_empty());
    assert_eq!(
        session.request(IDX_BATTERY, 1, NO_PARAMS).unwrap_err(),
        SessionError::NeedsRediscovery
    );
}

#[test]
fn hidden_features_and_writes_are_refused_before_any_write() {
    let (mut session, handle) = discovered_session();
    let writes_before = handle.borrow().writes.len();

    // Index 13 is 0x1802, type 0x70 (hidden) in the fixture table.
    let err = session.request(13, 0, NO_PARAMS).unwrap_err();
    assert!(
        matches!(err, SessionError::Policy(PolicyError::HardDenied { .. })),
        "{err:?}"
    );
    let err = session
        .request_feature(FeatureId(0x1802), 0, NO_PARAMS)
        .unwrap_err();
    assert!(matches!(
        err,
        SessionError::Policy(PolicyError::HardDenied { .. })
    ));

    // Index not in the table.
    let err = session.request(99, 0, NO_PARAMS).unwrap_err();
    assert_eq!(
        err,
        SessionError::Policy(PolicyError::UnknownFeatureIndex { feature_index: 99 })
    );

    // Volatile setters are not enabled on this session.
    for (feature, function) in [(FeatureId::ADJUSTABLE_DPI, 3), (FeatureId::REPORT_RATE, 2)] {
        let mut params = [0u8; 16];
        params[0] = 1;
        params[2] = 100;
        let err = session
            .request_feature(feature, function, params)
            .unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Policy(PolicyError::NotAllowlisted { .. })
            ),
            "{err:?}"
        );
    }

    // Onboard write/control functions and out-of-range sectors.
    // SetOnboardMode (fn 1) is a volatile-tier call: the read-only session
    // refuses it as not allowlisted.
    let err = session
        .request_feature(FeatureId::ONBOARD_PROFILES, 1, NO_PARAMS)
        .unwrap_err();
    assert!(
        matches!(
            err,
            SessionError::Policy(PolicyError::NotAllowlisted { .. })
        ),
        "{err:?}"
    );
    for function in [9u8, 10] {
        let err = session
            .request_feature(FeatureId::ONBOARD_PROFILES, function, NO_PARAMS)
            .unwrap_err();
        assert!(
            matches!(err, SessionError::Policy(PolicyError::HardDenied { .. })),
            "fn {function}: {err:?}"
        );
    }
    // BeginWrite/WriteData/EndWrite belong to the onboard write tier, which
    // this session does not enable.
    for function in [3u8, 6, 7, 8] {
        let err = session
            .request_feature(FeatureId::ONBOARD_PROFILES, function, NO_PARAMS)
            .unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Policy(PolicyError::NotAllowlisted { .. })
            ),
            "fn {function}: {err:?}"
        );
    }
    let mut params = [0u8; 16];
    params[..2].copy_from_slice(&0x0106u16.to_be_bytes());
    let err = session
        .request_feature(FeatureId::ONBOARD_PROFILES, 5, params)
        .unwrap_err();
    assert_eq!(
        err,
        SessionError::Policy(PolicyError::SectorNotAllowed { sector: 0x0106 })
    );

    // Invalid function nibble.
    let err = session.request(IDX_BATTERY, 16, NO_PARAMS).unwrap_err();
    assert_eq!(err, SessionError::Policy(PolicyError::InvalidFunction(16)));

    assert_eq!(
        handle.borrow().writes.len(),
        writes_before,
        "a refused request reached the link"
    );
}

#[test]
fn absent_feature_is_reported_without_a_write() {
    let (mut session, handle) = discovered_session();
    let writes_before = handle.borrow().writes.len();
    let err = session
        .request_feature(FeatureId(0x1234), 0, NO_PARAMS)
        .unwrap_err();
    assert_eq!(err, SessionError::FeatureAbsent(FeatureId(0x1234)));
    assert_eq!(handle.borrow().writes.len(), writes_before);
}

#[test]
fn software_id_is_validated_by_the_policy_and_used_on_the_wire() {
    let (mut session, handle) = discovered_session();
    for bad in [0u8, 1, 16, 255] {
        let err = session.set_sw_id(bad).unwrap_err();
        assert_eq!(
            err,
            SessionError::Policy(PolicyError::InvalidSwId(bad)),
            "{bad}"
        );
    }
    assert_eq!(session.sw_id(), hidpp::DEFAULT_SW_ID);

    session.set_sw_id(0x0c).expect("valid");
    {
        let mut state = handle.borrow_mut();
        // A reply to the old software ID first, then the real one.
        state.override_reply = Some(vec![
            battery_reply(),
            long(&[0x11, 0x01, IDX_BATTERY, 0x1c, 0x42, 0x04]),
        ]);
    }
    let payload = session
        .request_feature(FeatureId::UNIFIED_BATTERY, 1, NO_PARAMS)
        .expect("reply");
    assert_eq!(payload[0], 0x42);
    let state = handle.borrow();
    let last = state.writes.last().unwrap();
    assert_eq!(last[3], 0x1c);
    assert_ne!(*last, battery_tx());
}

fn read_data(sector: u16, offset: u16) -> [u8; 16] {
    hidpp::onboard::read_data_params(sector, offset)
}

/// The logged reply to the ReadData request for `sector` at `offset`.
fn logged_read_data_reply(sector: u16, offset: u16) -> Vec<u8> {
    let params = read_data(sector, offset);
    let mut tx = vec![0x11, 0x01, IDX_ONBOARD, 0x5b];
    tx.extend_from_slice(&params);
    logged_pairs()
        .into_iter()
        .find(|(t, _)| *t == tx)
        .map(|(_, rx)| rx)
        .expect("logged ReadData exchange")
}

#[test]
fn connection_notification_queued_before_a_request_blocks_the_write() {
    let (mut session, handle) = discovered_session();
    let writes_before = handle.borrow().writes.len();
    // The 0x41 arrived while nobody was waiting for a reply.
    handle.borrow_mut().inbox.push_back(hex("10014104000000"));
    let err = session.request(IDX_BATTERY, 1, NO_PARAMS).unwrap_err();
    assert_eq!(err, SessionError::NeedsRediscovery);
    assert_eq!(
        handle.borrow().writes.len(),
        writes_before,
        "a frame was sent"
    );
    assert!(session.table().is_empty());
    assert!(session.needs_rediscovery());
    let notes = session.drain_notifications();
    assert!(matches!(notes[0], Message::Hidpp10 { sub_id: 0x41, .. }));
    // Rediscovery works and is not poisoned by the drained frame.
    session.discover().expect("rediscover");
    assert_eq!(session.table().len(), 28);
}

#[test]
fn connection_notification_queued_before_discover_does_not_fail_it() {
    let (mut session, handle) = fixture_session();
    handle.borrow_mut().inbox.push_back(hex("10014104000000"));
    session.discover().expect("discover drops the stale 0x41");
    assert_eq!(session.table().len(), 28);
    assert!(!session.needs_rediscovery());
}

#[test]
fn stale_read_data_reply_is_not_taken_for_the_next_reply() {
    let (mut session, handle) = discovered_session();
    let feature = FeatureId::ONBOARD_PROFILES;

    // Sector 0 offset 0 goes unanswered and times out.
    handle.borrow_mut().override_reply = Some(vec![]);
    let err = session
        .request_feature(feature, 5, read_data(0, 0))
        .unwrap_err();
    assert_eq!(err, SessionError::Timeout);

    // Its reply arrives late. ReadData replies do not echo the offset, so
    // without a drain it would match the next ReadData request.
    let stale = logged_read_data_reply(0, 0);
    handle.borrow_mut().inbox.push_back(stale.clone());

    let payload = session
        .request_feature(feature, 5, read_data(1, 0))
        .expect("reply");
    let want = logged_read_data_reply(1, 0);
    assert_ne!(stale[4..], want[4..], "test needs two different payloads");
    assert_eq!(payload[..], want[4..], "the stale reply was taken");
}

/// 0x1004 broadcast as seen on the device: index 6, event 0, `37 04 00 00`.
fn battery_broadcast() -> Vec<u8> {
    long(&[0x11, 0x01, IDX_BATTERY, 0x00, 0x37, 0x04, 0x00, 0x00])
}

#[test]
fn poll_events_never_writes_and_buffers_notifications() {
    let (mut session, handle) = discovered_session();
    let writes_before = handle.borrow().writes.len();
    let requests_before = session.request_count();

    // Silence: returns after the timeout, no write.
    session
        .poll_events(Duration::from_millis(20))
        .expect("poll");
    assert!(session.drain_notifications().is_empty());

    {
        let mut state = handle.borrow_mut();
        // A reply to somebody else's request (stale), then the broadcast,
        // then a 0x8F error for a request we did not send.
        state
            .inbox
            .push_back(long(&[0x11, 0x01, IDX_BATTERY, 0x1c, 0x01, 0x01]));
        state.inbox.push_back(battery_broadcast());
        state.inbox.push_back(hex("10018f09110100"));
    }
    session
        .poll_events(Duration::from_millis(50))
        .expect("poll");

    let notes = session.drain_notifications();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let Message::Notification {
        feature_index,
        payload,
        ..
    } = &notes[0]
    else {
        panic!("not a notification: {:?}", notes[0]);
    };
    assert_eq!(*feature_index, IDX_BATTERY);
    assert_eq!(payload[..4], [0x37, 0x04, 0x00, 0x00]);
    assert!(!session.needs_rediscovery());
    assert_eq!(session.link_state(), None);
    assert!(handle.borrow().inbox.is_empty());
    assert_eq!(handle.borrow().writes.len(), writes_before, "poll wrote");
    assert_eq!(session.request_count(), requests_before);
}

#[test]
fn poll_events_follows_the_link_state_from_0x41() {
    let (mut session, handle) = discovered_session();
    assert_eq!(session.link_state(), None);

    // Another device behind the receiver says nothing about ours: the
    // notification is kept, the table and the link state stay.
    handle.borrow_mut().inbox.push_back(hex("10024111629f40"));
    session
        .poll_events(Duration::from_millis(50))
        .expect("poll");
    assert_eq!(session.link_state(), None);
    assert!(!session.needs_rediscovery());
    assert_eq!(session.table().len(), 28);
    let notes = session.drain_notifications();
    assert!(matches!(
        notes[0],
        Message::Hidpp10 {
            device_index: 2,
            sub_id: 0x41,
            ..
        }
    ));

    // Switched off: `11 62 9f 40`, bit 0x40 of payload byte 1 set.
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    session
        .poll_events(Duration::from_millis(50))
        .expect("poll");
    assert_eq!(session.link_state(), Some(false));
    assert!(session.needs_rediscovery());
    assert!(session.table().is_empty());
    let notes = session.drain_notifications();
    assert!(matches!(notes[0], Message::Hidpp10 { sub_id: 0x41, .. }));

    // Polling works with the table invalid and still writes nothing.
    let writes_before = handle.borrow().writes.len();
    session
        .poll_events(Duration::from_millis(10))
        .expect("poll");
    assert_eq!(handle.borrow().writes.len(), writes_before);

    // Switched on: `11 a2 9f 40`, bit clear.
    handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
    session
        .poll_events(Duration::from_millis(50))
        .expect("poll");
    assert_eq!(session.link_state(), Some(true));
    assert!(session.needs_rediscovery());
    assert_eq!(handle.borrow().writes.len(), writes_before);

    session.discover().expect("rediscover");
    assert!(!session.needs_rediscovery());
    assert_eq!(session.link_state(), Some(true));
    assert_eq!(session.table().len(), 28);
}

#[test]
fn other_device_connection_notification_does_not_abort_our_request() {
    let (mut session, handle) = discovered_session();
    // Another device's 0x41 (here: link down) arrives before our reply.
    handle.borrow_mut().pre_reply = vec![hex("10024111629f40")];
    let payload = session
        .request_feature(FeatureId::UNIFIED_BATTERY, 1, NO_PARAMS)
        .expect("reply");
    assert_eq!(payload[0], 0x37);
    assert!(!session.needs_rediscovery());
    assert_eq!(session.link_state(), None);
    // Also when it was queued before the request: nothing is aborted.
    handle.borrow_mut().inbox.push_back(hex("10024111629f40"));
    session
        .request_feature(FeatureId::UNIFIED_BATTERY, 1, NO_PARAMS)
        .expect("reply");
    let notes = session.drain_notifications();
    assert_eq!(notes.len(), 2);
}

#[test]
fn discover_does_not_write_while_the_link_is_down() {
    let (mut session, handle) = fixture_session();
    // Our link-down notification is queued when discover starts.
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    let err = session.discover().unwrap_err();
    assert_eq!(err, SessionError::LinkDown);
    assert!(handle.borrow().writes.is_empty(), "discover wrote");
    assert!(session.needs_rediscovery());
    assert_eq!(session.link_state(), Some(false));

    // Once the link is up again, discovery works.
    handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
    session.discover().expect("discover after link up");
    assert_eq!(session.table().len(), 28);
}
