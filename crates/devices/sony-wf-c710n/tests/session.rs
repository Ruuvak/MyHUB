//! Sessions against a scripted headset built from the mdr fixtures. Nothing opens a device.

use std::collections::HashMap;
use std::time::Duration;

use mdr::messages::{self, Message};
use mdr::messages::{AmbientMode, NcAsmMode};
use mdr::{DataType, Table, decode_frame, encode_ack};
use myhub_rfcomm::MockLink;
use sony_wf_c710n::{Session, SessionError, Snapshot, read_snapshot};

const FIXTURES: [&str; 5] = [
    include_str!("../../../mdr/tests/fixtures/handshake.txt"),
    include_str!("../../../mdr/tests/fixtures/features.txt"),
    include_str!("../../../mdr/tests/fixtures/multipoint.txt"),
    include_str!("../../../mdr/tests/fixtures/toggles.txt"),
    include_str!("../../../mdr/tests/fixtures/eq-extended.txt"),
];

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
        .collect()
}

/// A wire frame built the long way: header, checksum, escape.
fn build(data_type: u8, seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = vec![data_type, seq];
    body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    body.extend_from_slice(payload);
    let sum = body.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    body.push(sum);
    let mut out = vec![0x3E];
    for b in body {
        match b {
            0x3C..=0x3E => out.extend_from_slice(&[0x3D, b & 0xEF]),
            _ => out.push(b),
        }
    }
    out.push(0x3C);
    out
}

type Key = (u8, Vec<u8>);

/// For each host request of the fixtures (first occurrence), the headset DATA frames
/// that followed it in the log. ACKs are left out: the responder makes its own.
fn replies() -> HashMap<Key, Vec<Key>> {
    let mut map: HashMap<Key, Vec<Key>> = HashMap::new();
    let mut current: Option<Key> = None;
    for text in FIXTURES {
        for line in text.lines() {
            let line = line.split('#').next().unwrap().trim();
            let Some((dir, rest)) = line.split_at_checked(1) else {
                continue;
            };
            if dir != ">" && dir != "<" {
                continue;
            }
            let frame = decode_frame(&hex(rest)).expect("fixture frame");
            let key = (frame.data_type.to_u8(), frame.payload);
            if dir == ">" {
                current = if map.contains_key(&key) {
                    None
                } else {
                    map.insert(key.clone(), Vec::new());
                    Some(key)
                };
            } else if frame.data_type != DataType::Ack
                && let Some(cur) = &current
            {
                map.get_mut(cur).unwrap().push(key);
            }
        }
    }
    map
}

/// A mock that ACKs every request and answers it with the logged reply frames.
fn replay_link() -> MockLink {
    let table = replies();
    let mut headset_seq = 0u8;
    let mut link = MockLink::new();
    link.set_responder(move |written| {
        let frame = decode_frame(written).expect("session writes valid frames");
        if frame.data_type == DataType::Ack {
            return Vec::new();
        }
        let mut out = vec![encode_ack((frame.seq & 1) ^ 1)];
        let key = (frame.data_type.to_u8(), frame.payload);
        for (dt, payload) in table.get(&key).into_iter().flatten() {
            out.push(build(*dt, headset_seq, payload));
            headset_seq ^= 1;
        }
        out
    });
    link
}

fn snapshot_run() -> (Snapshot, Vec<Vec<u8>>) {
    let mut session = Session::new(replay_link());
    // No reply is logged for the codec request: do not wait 3 s for it.
    session.set_reply_timeout(Duration::from_millis(60));
    let snapshot = read_snapshot(&mut session).expect("snapshot");
    (snapshot, session.transport().writes().to_vec())
}

#[test]
fn snapshot_from_the_fixtures_has_the_logged_values() {
    let (s, _) = snapshot_run();
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains("00:11:22"), "no address in the snapshot");
    assert_eq!(s.identity.model, "WF-C710N");
    assert_eq!(s.identity.firmware, "1.0.5");
    let b = s.battery.expect("battery");
    assert_eq!((b.left.level, b.right.level), (100, 100));
    assert_eq!(s.cradle.expect("cradle").cell.level, 0x32);
    let nc = s.ncasm.expect("NC/ASM");
    assert_eq!(nc.param.mode, NcAsmMode::NoiseCancelling);
    assert_eq!(nc.param.ambient_mode, AmbientMode::Normal);
    assert_eq!(nc.param.level, 0x14);
    let eq = s.eq.expect("EQ");
    assert_eq!(eq.param.preset, 0xA0);
    assert_eq!(eq.param.bands, [0x10, 0x0E, 0x0B, 0x0A, 0x0C, 0x0F]);
    assert_eq!((s.multipoint, s.sidetone), (Some(true), Some(true)));
    assert!(s.codec.is_none(), "no codec reply in the fixtures");
}

#[test]
fn session_writes_only_acks_and_read_tier_requests() {
    const READ_COMMANDS: [u8; 11] = [
        0x00, 0x04, 0x06, 0x12, 0x22, 0x50, 0x56, 0x5A, 0x60, 0x66, 0xD6,
    ];
    let (_, writes) = snapshot_run();
    assert!(writes.len() > 10);
    for w in writes {
        let f = decode_frame(&w).expect("valid frame");
        if f.data_type == DataType::Ack {
            continue;
        }
        assert!(
            matches!(f.data_type, DataType::DataMdr | DataType::DataMdrNo2),
            "{:?}",
            f.data_type
        );
        assert_eq!(f.payload.len(), 2, "{:02x?}", f.payload);
        assert!(READ_COMMANDS.contains(&f.payload[0]), "{:02x?}", f.payload);
        assert_ne!(f.payload, [0x02, 0x00], "capability info holds the address");
    }
}

#[test]
fn frames_pushed_before_the_first_send_are_acked() {
    let mut link = MockLink::new();
    link.push_incoming(build(
        0x0C,
        0,
        &[0x25, 0x09, 0x64, 0x00, 0x64, 0x00, 0x14, 0x14],
    ));
    link.push_incoming(build(0x0C, 1, &[0x25, 0x0A, 0x32, 0x00, 0x15]));
    let mut session = Session::new(link);
    session.poll(10).unwrap();
    session.poll(10).unwrap();
    assert_eq!(
        session.transport().writes(),
        [encode_ack(1), encode_ack(0)],
        "one ACK per pushed frame, seq flipped"
    );
    assert_eq!(session.take_notifications().len(), 2);
}

#[test]
fn reconnect_restarts_the_tables_and_refuses_requests_until_they_are_read_again() {
    let mut session = Session::new(replay_link());
    session.set_reply_timeout(Duration::from_millis(60));
    read_snapshot(&mut session).expect("first snapshot");
    assert!(session.functions().count(Table::One) > 0);

    session.reconnect(replay_link());
    assert_eq!(session.functions().count(Table::One), 0, "table restarts");
    let refused = session.request(&messages::battery_lr());
    assert!(
        matches!(refused, Err(SessionError::Policy(_))),
        "{refused:?}"
    );
    assert!(
        session
            .transport()
            .writes()
            .iter()
            .all(|w| decode_frame(w).is_ok_and(|f| f.data_type == DataType::Ack)),
        "the refused request sent no request frame"
    );
    // A fresh handshake on the new link works again.
    read_snapshot(&mut session).expect("snapshot after the reconnect");
}

#[test]
fn a_notification_between_request_and_reply_is_queued_not_taken_as_the_reply() {
    let table = replies();
    let mut link = MockLink::new();
    link.set_responder(move |written| {
        let frame = decode_frame(written).expect("valid frame");
        if frame.data_type == DataType::Ack {
            return Vec::new();
        }
        let mut out = vec![encode_ack((frame.seq & 1) ^ 1)];
        // Battery notification first (headset seq 0), then the logged reply (seq 1).
        out.push(build(
            0x0C,
            0,
            &[0x25, 0x09, 0x64, 0x00, 0x64, 0x00, 0x14, 0x14],
        ));
        let key = (frame.data_type.to_u8(), frame.payload);
        for (dt, payload) in table.get(&key).into_iter().flatten() {
            out.push(build(*dt, 1, payload));
        }
        out
    });
    let mut session = Session::new(link);
    let reply = session
        .request(&messages::protocol_info(Table::One))
        .expect("reply");
    assert!(
        matches!(reply.message, Message::ProtocolInfo(_)),
        "{reply:?}"
    );
    let queued = session.take_notifications();
    assert_eq!(queued.len(), 1);
    assert!(matches!(queued[0].message, Message::BatteryLr(_)));
}

#[test]
fn to_detail_maps_the_fixture_snapshot() {
    let (mut s, _) = snapshot_run();
    // Lower the right bud: the summary shows the lower of the two.
    let mut b = s.battery.expect("battery");
    b.right.level = 42;
    s.battery = Some(b);
    let d = sony_wf_c710n::to_detail(&s, true);
    assert_eq!(d.summary.id, "sony-wf-c710n");
    assert_eq!(d.summary.battery_percent, Some(42));
    assert!(d.read_only);
    let json = serde_json::to_value(&d).unwrap();
    assert_eq!(json["kind"], "headset");
    assert_eq!(json["protocol"], "Sony MDR");
    let h = &json["headset"];
    assert_eq!(h["battery"]["left"]["percent"], 100);
    assert_eq!(h["battery"]["left"]["charging"], "notCharging");
    assert_eq!(h["battery"]["case"]["percent"], 0x32);
    assert_eq!(h["ncAsm"]["mode"], "noiseCancelling");
    assert_eq!(h["ncAsm"]["ambientMode"], "normal");
    assert_eq!(h["ncAsm"]["level"], 20);
    assert_eq!(h["eq"]["preset"], 0xA0);
    assert_eq!(h["eq"]["presetName"], "custom");
    let info = h["eq"]["bandInfo"].as_array().expect("band info");
    assert_eq!(info[0]["kind"], "clearBass");
    assert_eq!(info[0]["hz"], serde_json::Value::Null);
    let hz: Vec<_> = info[1..]
        .iter()
        .map(|b| (b["kind"].as_str().unwrap(), b["hz"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        hz,
        [
            ("hz", 400),
            ("hz", 1000),
            ("hz", 2500),
            ("hz", 6300),
            ("hz", 16000)
        ]
    );
    let names: Vec<_> = h["eq"]["presets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        names.contains(&"off") && names.contains(&"user1"),
        "{names:?}"
    );
    assert_eq!(h["multipoint"], true);
    assert_eq!(h["firmware"], "1.0.5");
    assert_eq!(
        sony_wf_c710n::to_detail(&s, false).summary.connection,
        myhub_core::Connection::Disconnected
    );
}

#[test]
fn notifications_update_the_snapshot_or_ask_for_a_reread() {
    use sony_wf_c710n::{Applied, Item};
    let (mut s, _) = snapshot_run();
    // NC/ASM NTFY: ambient mode, voice, level 7 (69 17 changed total mode amb level).
    let ntfy = mdr::parse(Table::One, &[0x69, 0x17, 0x01, 0x01, 0x01, 0x01, 0x07]);
    assert_eq!(s.apply(&ntfy), Applied::Updated);
    let nc = s.ncasm.as_ref().unwrap();
    assert_eq!(nc.param.mode, NcAsmMode::Ambient);
    assert_eq!(nc.param.ambient_mode, AmbientMode::Voice);
    assert_eq!(nc.param.level, 7);
    // A battery NTFY replaces the values.
    let battery = mdr::parse(
        Table::One,
        &[0x25, 0x09, 0x50, 0x00, 0x4B, 0x01, 0x14, 0x14],
    );
    assert_eq!(s.apply(&battery), Applied::Updated);
    assert_eq!(s.battery.unwrap().right.level, 0x4B);
    // A bare `59 00` and an unparsable battery NTFY ask for a GET; other commands are ignored.
    let bare = mdr::parse(Table::One, &[0x59, 0x00]);
    assert_eq!(s.apply(&bare), Applied::Reread(Item::Eq));
    let odd = mdr::parse(Table::One, &[0x25, 0x7F]);
    assert_eq!(s.apply(&odd), Applied::Reread(Item::Battery));
    let other = mdr::parse(Table::One, &[0xA5, 0x01]);
    assert_eq!(s.apply(&other), Applied::Ignored);
}
