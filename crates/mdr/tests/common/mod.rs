//! Shared helpers. Nothing here opens a device. The frame builder is written
//! independently of the crate so tests do not check the encoder against itself.
#![allow(dead_code)]

use mdr::{
    AlertToken, CheckedRequest, DataType, DeviceLimits, Frame, FrameDecoder, FunctionTable,
    Incoming, Link, LinkEvent, Message, Policy, ReceivedFrame, Table, messages, parse_received,
};

pub fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
        .collect()
}

/// Build a wire frame the long way: header, checksum, escape.
pub fn build(data_type: u8, seq: u8, payload: &[u8]) -> Vec<u8> {
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

pub fn data1(seq: u8, payload: &[u8]) -> Vec<u8> {
    build(0x0C, seq, payload)
}

pub fn data2(seq: u8, payload: &[u8]) -> Vec<u8> {
    build(0x0E, seq, payload)
}

pub fn ack(seq: u8) -> Vec<u8> {
    build(0x01, seq, &[])
}

/// One fixture line: `(host_to_headset, wire bytes)`.
pub type FixtureFrame = (bool, Vec<u8>);

pub const HANDSHAKE: &str = include_str!("../fixtures/handshake.txt");
pub const FEATURES: &str = include_str!("../fixtures/features.txt");
pub const NCASM_SET: &str = include_str!("../fixtures/ncasm-set.txt");
pub const NCASM_RESTORE: &str = include_str!("../fixtures/ncasm-restore.txt");
pub const TOGGLES: &str = include_str!("../fixtures/toggles.txt");
pub const MULTIPOINT: &str = include_str!("../fixtures/multipoint.txt");
pub const EQ_BANDS: &str = include_str!("../fixtures/eq-bands.txt");
pub const EQ_PRESET: &str = include_str!("../fixtures/eq-preset.txt");
pub const EQ_RESTORE: &str = include_str!("../fixtures/eq-restore.txt");

pub const ALL_FIXTURES: [&str; 9] = [
    HANDSHAKE,
    FEATURES,
    NCASM_SET,
    NCASM_RESTORE,
    TOGGLES,
    MULTIPOINT,
    EQ_BANDS,
    EQ_PRESET,
    EQ_RESTORE,
];

pub fn load(text: &str) -> Vec<FixtureFrame> {
    text.lines()
        .filter_map(|l| {
            let l = l.split('#').next().unwrap().trim();
            let (dir, rest) = l.split_at_checked(1)?;
            match dir {
                ">" => Some((true, hex(rest))),
                "<" => Some((false, hex(rest))),
                _ => None,
            }
        })
        .collect()
}

/// Decode every headset to host frame of a fixture through the streaming decoder.
pub fn headset_frames(text: &str) -> Vec<Frame> {
    let mut dec = FrameDecoder::new();
    let mut out = Vec::new();
    for (tx, wire) in load(text) {
        if !tx {
            out.extend(dec.push(&wire));
        }
    }
    assert_eq!(dec.dropped(), 0, "fixture contains a bad frame");
    out
}

/// Every headset to host frame of a fixture as the link delivers it.
pub fn headset_received(text: &str) -> Vec<ReceivedFrame> {
    let mut link = Link::new();
    let mut out = Vec::new();
    for (tx, wire) in load(text) {
        if !tx {
            for e in link.on_bytes(&wire, 0) {
                if let LinkEvent::Delivered(f) = e {
                    out.push(f);
                }
            }
        }
    }
    out
}

/// Every parsed data frame of a fixture, headset to host.
pub fn headset_messages(text: &str) -> Vec<Incoming> {
    headset_received(text)
        .into_iter()
        .map(parse_received)
        .collect()
}

thread_local! {
    /// Handshake and features fixtures through ONE link, so table and limits share an epoch.
    static SESSION: (FunctionTable, DeviceLimits, std::cell::RefCell<Link>) = build_session();
}

fn build_session() -> (FunctionTable, DeviceLimits, std::cell::RefCell<Link>) {
    let mut link = Link::new();
    let mut ft = FunctionTable::new();
    let mut limits = DeviceLimits::new();
    for text in [HANDSHAKE, FEATURES] {
        for (tx, wire) in load(text) {
            if tx {
                continue;
            }
            for e in link.on_bytes(&wire, 0) {
                if let LinkEvent::Delivered(f) = e {
                    ft.absorb(&f);
                    limits.absorb(&f);
                }
            }
        }
    }
    (ft, limits, std::cell::RefCell::new(link))
}

/// Function table from the handshake fixture.
pub fn function_table() -> FunctionTable {
    SESSION.with(|s| s.0.clone())
}

/// Limits from the features fixture (same epoch as [`function_table`]).
pub fn session_limits() -> DeviceLimits {
    SESSION.with(|s| s.1.clone())
}

/// `Policy::check` with the epoch of the table or limits in use.
pub trait CheckExt {
    fn check_e(
        &self,
        ft: &FunctionTable,
        limits: &DeviceLimits,
        req: &mdr::Request,
    ) -> Result<CheckedRequest, mdr::PolicyError>;
}

impl CheckExt for Policy {
    fn check_e(
        &self,
        ft: &FunctionTable,
        limits: &DeviceLimits,
        req: &mdr::Request,
    ) -> Result<CheckedRequest, mdr::PolicyError> {
        let epoch = ft.epoch().or(limits.epoch()).unwrap_or(0);
        self.check(epoch, ft, limits, req)
    }
}

/// Function table made from received support function replies (table 1, table 2).
pub fn ft_with(t1: &[(u8, u8)], t2: &[(u8, u8)]) -> FunctionTable {
    let mut link = Link::new();
    let mut ft = FunctionTable::new();
    for (i, (dt, entries)) in [(0x0Cu8, t1), (0x0E, t2)].into_iter().enumerate() {
        let mut payload = vec![0x07, 0x00, entries.len() as u8];
        for (id, v) in entries {
            payload.extend([*id, *v]);
        }
        for e in link.on_bytes(&build(dt, i as u8, &payload), 0) {
            if let LinkEvent::Delivered(f) = e {
                ft.absorb(&f);
            }
        }
    }
    ft
}

/// The wire bytes of a request checked against the session link ([`function_table`] and
/// [`session_limits`] come from it), sent with sequence bit `seq`.
pub fn wire_of(policy: &Policy, req: &mdr::Request, seq: u8) -> Vec<u8> {
    SESSION.with(|s| {
        let mut link = s.2.borrow_mut();
        let ack_for = |link: &Link| ack(link.next_seq() ^ 1);
        if link.next_seq() != seq & 1 {
            let dummy = Policy::new()
                .check(
                    link.epoch(),
                    &FunctionTable::new(),
                    &DeviceLimits::new(),
                    &messages::protocol_info(Table::One),
                )
                .unwrap();
            link.send(dummy, 0).unwrap();
            let a = ack_for(&link);
            link.on_bytes(&a, 0);
        }
        let c = policy
            .check(link.epoch(), &s.0, &s.1, req)
            .expect("request passes the policy");
        let wire = link.send(c, 0).unwrap();
        let a = ack_for(&link);
        link.on_bytes(&a, 0);
        wire
    })
}

/// A link that received `99 00 <id> <action>`, and the token of that alert.
pub fn link_with_token(id: u8, action: u8) -> (Link, Option<AlertToken>) {
    let mut link = Link::new();
    let mut token = None;
    for e in link.on_bytes(&data1(0, &[0x99, 0x00, id, action]), 0) {
        if let LinkEvent::Delivered(f) = e
            && let Message::Alert(mut a) = parse_received(f).message
        {
            token = a.take_token();
        }
    }
    (link, token)
}

/// Table (table 1 entries) and limits from crafted headset payloads, one link.
pub fn session_with(t1: &[(u8, u8)], caps: &[&[u8]]) -> (FunctionTable, DeviceLimits) {
    let mut link = Link::new();
    let mut ft = FunctionTable::new();
    let mut limits = DeviceLimits::new();
    let mut sup = vec![0x07, 0x00, t1.len() as u8];
    for (id, v) in t1 {
        sup.extend([*id, *v]);
    }
    let mut payloads: Vec<&[u8]> = vec![&sup];
    payloads.extend_from_slice(caps);
    for (i, p) in payloads.iter().enumerate() {
        for e in link.on_bytes(&data1(i as u8 & 1, p), 0) {
            if let LinkEvent::Delivered(f) = e {
                ft.absorb(&f);
                limits.absorb(&f);
            }
        }
    }
    (ft, limits)
}

pub fn is_data(dt: DataType) -> bool {
    matches!(dt, DataType::DataMdr | DataType::DataMdrNo2)
}
