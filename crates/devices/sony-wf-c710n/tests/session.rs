//! Sessions against a scripted headset built from the mdr fixtures. Nothing opens a device.

use std::collections::HashMap;
use std::time::Duration;

use mdr::messages::{AmbientMode, NcAsmMode};
use mdr::{DataType, decode_frame, encode_ack};
use myhub_rfcomm::MockLink;
use sony_wf_c710n::{Session, Snapshot, read_snapshot};

const FIXTURES: [&str; 4] = [
    include_str!("../../../mdr/tests/fixtures/handshake.txt"),
    include_str!("../../../mdr/tests/fixtures/features.txt"),
    include_str!("../../../mdr/tests/fixtures/multipoint.txt"),
    include_str!("../../../mdr/tests/fixtures/toggles.txt"),
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
    const READ_COMMANDS: [u8; 10] = [0x00, 0x04, 0x06, 0x12, 0x22, 0x50, 0x56, 0x60, 0x66, 0xD6];
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
