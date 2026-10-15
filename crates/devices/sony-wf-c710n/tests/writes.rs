//! The write path against a scripted headset. Handshake and capability replies come from
//! the mdr fixtures; the settings the writes touch are simulated, including the quirks
//! seen on the real headset (bands sent while another preset is active are ignored).
//! Nothing opens a device.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use mdr::messages::{self, AmbientMode, NcAsmMode};
use mdr::policy::Tier;
use mdr::{DataType, PolicyError, decode_frame, encode_ack};
use myhub_rfcomm::MockLink;
use sony_wf_c710n::{MultipointOutcome, Session, SessionError, WriteError, read_snapshot};

const FIXTURES: [&str; 4] = [
    include_str!("../../../mdr/tests/fixtures/handshake.txt"),
    include_str!("../../../mdr/tests/fixtures/features.txt"),
    include_str!("../../../mdr/tests/fixtures/multipoint.txt"),
    include_str!("../../../mdr/tests/fixtures/toggles.txt"),
];

const CUSTOM_BANDS: [u8; 6] = [0x10, 0x0E, 0x0B, 0x0A, 0x0C, 0x0F];
const BASS_BANDS: [u8; 6] = [0x11, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A];

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
        .collect()
}

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

/// Headset DATA frames that followed each host request in the fixtures.
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

/// The simulated settings and how the headset misbehaves.
struct Sim {
    eq_preset: u8,
    eq_bands: Vec<u8>,
    custom: Vec<u8>,
    nc: [u8; 5],
    sidetone: u8,
    multipoint: u8,
    pending_multipoint: u8,
    /// Accept NC/ASM SETs without applying them and without a notification.
    ignore_ncasm_set: bool,
    /// After CUSTOM is selected, follow the next EQ read with a notification that says
    /// BASS is active.
    inject_bass_after_custom_get: bool,
}

impl Sim {
    fn new() -> Self {
        Self {
            eq_preset: 0xA0,
            eq_bands: CUSTOM_BANDS.to_vec(),
            custom: CUSTOM_BANDS.to_vec(),
            nc: [0x01, 0x01, 0x00, 0x00, 0x14],
            sidetone: 0,
            multipoint: 0,
            pending_multipoint: 0,
            ignore_ncasm_set: false,
            inject_bass_after_custom_get: false,
        }
    }

    fn eq_frame(&self, command: u8) -> Vec<u8> {
        let mut p = vec![command, 0x00, self.eq_preset, 0x06];
        p.extend_from_slice(&self.eq_bands);
        p
    }
}

/// Link whose responder ACKs every request and answers: simulated settings first, the
/// fixture replies for everything else. Returns the flag that drops the link.
fn scripted(mut sim: Sim) -> (MockLink, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let table = replies();
    let mut seq = 0u8;
    let mut link = MockLink::new();
    let drop_flag = link.disconnect_flag();
    let flag = std::sync::Arc::clone(&drop_flag);
    link.set_responder(move |written| {
        let frame = decode_frame(written).expect("session writes valid frames");
        if frame.data_type == DataType::Ack {
            return Vec::new();
        }
        let mut out = vec![encode_ack((frame.seq & 1) ^ 1)];
        let mut data: Vec<Vec<u8>> = Vec::new();
        match frame.payload.as_slice() {
            [0x56, 0x00] => {
                data.push(sim.eq_frame(0x57));
                if sim.inject_bass_after_custom_get && sim.eq_preset == 0xA0 {
                    sim.inject_bass_after_custom_get = false;
                    data.push([&[0x59, 0x00, 0x16, 0x06][..], &BASS_BANDS[..]].concat());
                }
            }
            [0x58, 0x00, id, 0x00] => {
                sim.eq_preset = *id;
                sim.eq_bands = if *id == 0xA0 {
                    sim.custom.clone()
                } else {
                    BASS_BANDS.to_vec()
                };
                data.push(sim.eq_frame(0x59));
            }
            [0x58, 0x00, id, 0x06, steps @ ..] => {
                if *id == 0xA0 && sim.eq_preset == 0xA0 {
                    sim.custom = steps.to_vec();
                    sim.eq_bands = steps.to_vec();
                    data.push(sim.eq_frame(0x59));
                } else {
                    data.push(vec![0x59, 0x00]);
                }
            }
            [0x66, 0x17] => data.push([&[0x67, 0x17][..], &sim.nc].concat()),
            [0x68, 0x17, rest @ ..] => {
                if !sim.ignore_ncasm_set {
                    sim.nc.copy_from_slice(rest);
                    data.push([&[0x69, 0x17][..], &sim.nc].concat());
                }
            }
            [0xD6, 0xD2] => data.push(vec![0xD7, 0xD2, 0x00, sim.sidetone]),
            [0xD6, 0xD1] => data.push(vec![0xD7, 0xD1, 0x00, sim.multipoint]),
            [0xD8, 0xD2, 0x00, v] => {
                sim.sidetone = *v;
                data.push(vec![0xD9, 0xD2, 0x00, *v]);
            }
            [0xD8, 0xD1, 0x00, v] => {
                sim.pending_multipoint = *v;
                data.push(vec![0x99, 0x00, 0x07, 0x01]);
            }
            [0x98, 0x00, 0x07, 0x01] => {
                sim.multipoint = sim.pending_multipoint;
                flag.store(true, Ordering::SeqCst);
            }
            _ => {
                let key = (frame.data_type.to_u8(), frame.payload.clone());
                for (dt, payload) in table.get(&key).into_iter().flatten() {
                    out.push(build(*dt, seq, payload));
                    seq ^= 1;
                }
            }
        }
        for payload in data {
            out.push(build(0x0C, seq, &payload));
            seq ^= 1;
        }
        out
    });
    (link, drop_flag)
}

fn session(
    sim: Sim,
) -> (
    Session<MockLink>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let (link, drop_flag) = scripted(sim);
    let mut session = Session::new(link);
    session.set_reply_timeout(Duration::from_millis(100));
    read_snapshot(&mut session).expect("snapshot");
    session.set_verify_wait(Duration::from_millis(100));
    (session, drop_flag)
}

/// Payloads of the request frames written so far (ACKs left out).
fn sent(session: &Session<MockLink>) -> Vec<Vec<u8>> {
    session
        .transport()
        .writes()
        .iter()
        .map(|w| decode_frame(w).expect("valid frame"))
        .filter(|f| f.data_type != DataType::Ack)
        .map(|f| f.payload)
        .collect()
}

#[test]
fn a_write_leaves_no_gate_open_and_the_read_tier_refuses_setters() {
    let (mut s, _) = session(Sim::new());
    let change = s.select_eq_preset(0x16).expect("preset");
    assert_eq!((change.before.preset, change.after.preset), (0xA0, 0x16));
    let frames = s.transport().writes().len();
    // The session policy is still the read tier: the same SET is refused, nothing is sent.
    let refused = s.request(&messages::eq_select_preset(0x17));
    assert!(
        matches!(
            refused,
            Err(SessionError::Policy(PolicyError::TierNotEnabled {
                tier: Tier::Setting
            }))
        ),
        "{refused:?}"
    );
    assert_eq!(s.transport().writes().len(), frames);
}

#[test]
fn bands_select_custom_first_when_another_preset_is_active() {
    let mut sim = Sim::new();
    sim.eq_preset = 0x16;
    sim.eq_bands = BASS_BANDS.to_vec();
    let (mut s, _) = session(sim);
    let steps = [9, 10, 11, 12, 10, 10];
    let change = s.set_eq_bands(&steps).expect("bands");
    assert_eq!(change.before.preset, 0x16);
    assert_eq!(change.custom_before, CUSTOM_BANDS);
    assert_eq!(
        (change.after.preset, change.after.bands),
        (0xA0, steps.to_vec())
    );
    let eq_sets: Vec<Vec<u8>> = sent(&s).into_iter().filter(|p| p[0] == 0x58).collect();
    assert_eq!(
        eq_sets,
        [
            vec![0x58, 0x00, 0xA0, 0x00],
            [&[0x58, 0x00, 0xA0, 0x06][..], &steps].concat()
        ],
        "CUSTOM is selected before any bands go out"
    );
}

#[test]
fn multipoint_sends_one_positive_answer_and_asks_for_a_reconnect() {
    let (mut s, drop_flag) = session(Sim::new());
    assert!(!drop_flag.load(Ordering::SeqCst));
    let outcome = s.set_multipoint(false).expect("multipoint");
    assert_eq!(
        outcome,
        MultipointOutcome::PendingReconnect { before: true }
    );
    let frames = sent(&s);
    let answers = frames.iter().filter(|p| p[0] == 0x98).count();
    assert_eq!(answers, 1);
    assert!(frames.contains(&vec![0x98, 0x00, 0x07, 0x01]));
    assert!(frames.contains(&vec![0xD8, 0xD1, 0x00, 0x01]));
}

#[test]
fn a_value_the_headset_did_not_keep_is_a_verify_failure() {
    let mut sim = Sim::new();
    sim.ignore_ncasm_set = true;
    let (mut s, _) = session(sim);
    let result = s.set_nc_asm(NcAsmMode::Ambient, AmbientMode::Voice, 5);
    assert_eq!(result, Err(WriteError::VerifyFailed));
}

#[test]
fn out_of_range_values_are_refused_before_any_frame() {
    let (mut s, _) = session(Sim::new());
    let written = s.transport().writes().len();
    assert!(matches!(
        s.set_nc_asm(NcAsmMode::Ambient, AmbientMode::Normal, 99),
        Err(WriteError::OutOfRange(_))
    ));
    assert!(matches!(
        s.set_eq_bands(&[10; 5]),
        Err(WriteError::OutOfRange(_))
    ));
    assert!(matches!(
        s.set_eq_bands(&[10, 10, 10, 10, 10, 21]),
        Err(WriteError::OutOfRange(_))
    ));
    assert!(matches!(
        s.select_eq_preset(0x42),
        Err(WriteError::OutOfRange(_))
    ));
    assert_eq!(
        s.transport().writes().len(),
        written,
        "no frame, not even a read"
    );
}

#[test]
fn bands_are_not_sent_when_a_notification_says_another_preset_is_active() {
    let mut sim = Sim::new();
    sim.eq_preset = 0x16;
    sim.eq_bands = BASS_BANDS.to_vec();
    sim.inject_bass_after_custom_get = true;
    let (mut s, _) = session(sim);
    let result = s.set_eq_bands(&[9, 10, 11, 12, 10, 10]);
    assert_eq!(result, Err(WriteError::VerifyFailed));
    assert!(
        !sent(&s)
            .iter()
            .any(|p| p[..2] == [0x58, 0x00] && p.get(3) == Some(&0x06)),
        "no bands frame went out"
    );
}
