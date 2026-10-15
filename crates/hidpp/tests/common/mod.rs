//! Shared fixture loading. Nothing here opens a device.
#![allow(dead_code)]

use hidpp::onboard::OnboardInfo;
use serde_json::Value;

pub fn hex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
        .collect()
}

pub fn fixture() -> Value {
    let text = include_str!("../fixtures/hidpp_read.json");
    serde_json::from_str(text).expect("fixture json")
}

pub fn fixture_bytes(key: &str) -> Vec<u8> {
    hex(fixture()[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing {key}")))
}

pub fn sector(name: &str) -> Vec<u8> {
    hex(fixture()["sectors"][name]
        .as_str()
        .unwrap_or_else(|| panic!("missing sector {name}")))
}

pub fn info() -> OnboardInfo {
    OnboardInfo::parse(&fixture_bytes("8100_fn0_info")).expect("info")
}

/// One logged frame: `(is_tx, 20 raw bytes)`.
pub fn frames() -> Vec<(bool, Vec<u8>)> {
    include_str!("../fixtures/frames.log")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (tag, rest) = l.split_once(' ').unwrap_or((l, ""));
            match tag {
                "TX" => (true, hex(rest.trim())),
                "RX" => (false, hex(rest.trim())),
                other => panic!("unexpected log tag {other}"),
            }
        })
        .collect()
}

/// Feature table built from the fixture `features` list (indexes 1..=28).
pub fn table_from_json() -> hidpp::FeatureTable {
    use hidpp::{FeatureEntry, FeatureId, FeatureTable};
    let json = fixture();
    let mut table = FeatureTable::new();
    for j in json["features"].as_array().expect("features") {
        let id = u16::from_str_radix(j["id"].as_str().unwrap().trim_start_matches("0x"), 16)
            .expect("feature id");
        table
            .insert_entry(FeatureEntry {
                index: j["index"].as_u64().unwrap() as u8,
                id: FeatureId(id),
                feature_type: j["type"].as_u64().unwrap() as u8,
                version: j["version"].as_u64().unwrap() as u8,
            })
            .expect("insert");
    }
    table
}
