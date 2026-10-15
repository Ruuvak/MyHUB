mod common;

use common::{fixture, frames};
use hidpp::feature::*;
use hidpp::frame::{DEFAULT_SW_ID, Message, decode};
use hidpp::{Policy, PolicyError};

#[test]
fn parsers() {
    let r = parse_root_get_feature(&[0x07, 0x00, 0x02]).unwrap();
    assert_eq!((r.index, r.feature_type, r.version), (7, 0, 2));
    assert!(r.is_present());
    assert!(!parse_root_get_feature(&[0, 0, 0]).unwrap().is_present());
    assert!(parse_root_get_feature(&[1, 2]).is_err());

    assert_eq!(parse_feature_count(&[0x1C]).unwrap(), 28);
    assert!(parse_feature_count(&[]).is_err());

    let f = parse_feature_id(&[0x22, 0x01, 0x00, 0x02]).unwrap();
    assert_eq!(f.id, FeatureId::ADJUSTABLE_DPI);
    assert_eq!((f.feature_type, f.version), (0, 2));
    assert!(parse_feature_id(&[0x22, 0x01, 0x00]).is_err());

    assert_eq!(
        root_get_feature_params(FeatureId(0x8100))[..3],
        [0x81, 0x00, 0]
    );
    assert_eq!(feature_set_get_feature_id_params(9)[0], 9);
}

fn id_of(s: &str) -> FeatureId {
    FeatureId(u16::from_str_radix(s.trim_start_matches("0x"), 16).unwrap())
}

/// Replays the logged Root and FeatureSet traffic into a table, resolving the
/// FeatureSet index from the Root reply instead of hardcoding it.
fn rebuild() -> FeatureTable {
    let f = frames();
    let mut table = FeatureTable::new();
    let mut feature_set_index = None;
    let mut count = None;
    // feature index, function, params of the last TX
    let mut pending: Option<(u8, u8, Vec<u8>)> = None;
    for (is_tx, raw) in &f {
        let Message::Response {
            feature_index,
            function,
            payload,
            ..
        } = decode(raw).unwrap()
        else {
            panic!("unexpected frame")
        };
        if *is_tx {
            pending = Some((feature_index, function, payload.to_vec()));
            continue;
        }
        let (req_feat, req_fn, req_params) = pending.take().unwrap();
        if req_feat == 0 && req_fn == ROOT_FN_GET_FEATURE && feature_set_index.is_none() {
            assert_eq!(req_params[..2], [0x00, 0x01]);
            let r = parse_root_get_feature(&payload).unwrap();
            assert!(r.is_present());
            feature_set_index = Some(r.index);
            continue;
        }
        let Some(fs) = feature_set_index else {
            continue;
        };
        if req_feat != fs {
            continue;
        }
        match req_fn {
            FEATURE_SET_FN_GET_COUNT => count = Some(parse_feature_count(&payload).unwrap()),
            FEATURE_SET_FN_GET_FEATURE_ID => table
                .insert(req_params[0], parse_feature_id(&payload).unwrap())
                .unwrap(),
            _ => {}
        }
    }
    assert_eq!(count, Some(28));
    table
}

#[test]
fn table_rebuilt_from_frames_equals_json() {
    let table = rebuild();
    let json = fixture();
    let features = json["features"].as_array().unwrap();
    assert_eq!(
        json["feature_count"].as_u64().unwrap() as usize,
        features.len()
    );
    assert_eq!(table.len(), features.len());
    for (entry, j) in table.entries().iter().zip(features) {
        assert_eq!(u64::from(entry.index), j["index"].as_u64().unwrap());
        assert_eq!(entry.id, id_of(j["id"].as_str().unwrap()));
        assert_eq!(u64::from(entry.feature_type), j["type"].as_u64().unwrap());
        assert_eq!(u64::from(entry.version), j["version"].as_u64().unwrap());
    }
}

#[test]
fn table_lookups() {
    let table = rebuild();
    assert_eq!(table.index_of(FeatureId::ROOT), Some(0));
    assert_eq!(table.index_of(FeatureId::FEATURE_SET), Some(1));
    assert_eq!(table.index_of(FeatureId::UNIFIED_BATTERY), Some(6));
    let dpi = table.by_id(FeatureId::ADJUSTABLE_DPI).unwrap();
    assert_eq!((dpi.index, dpi.version), (7, 2));
    assert_eq!(table.by_index(11).unwrap().id, FeatureId::REPORT_RATE);
    assert_eq!(table.by_index(9).unwrap().id, FeatureId::ONBOARD_PROFILES);
    assert!(table.by_id(FeatureId(0x1B04)).is_none());
    assert!(table.by_index(99).is_none());
    assert!(!table.is_empty());
}

#[test]
fn insert_replaces_same_index() {
    let mut t = FeatureTable::new();
    let r = |id| FeatureIdReply {
        id: FeatureId(id),
        feature_type: 0,
        version: 0,
    };
    t.insert(3, r(0x1111)).unwrap();
    t.insert(3, r(0x2222)).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t.by_index(3).unwrap().id, FeatureId(0x2222));
}

#[test]
fn hidden_entries_are_rejected_for_every_function() {
    let table = rebuild();
    let mut hidden = 0;
    for e in table.entries() {
        if e.feature_type & 0x60 == 0 {
            continue;
        }
        hidden += 1;
        for policy in [Policy::new(), Policy::new().with_volatile_writes()] {
            for function in 0..=15u8 {
                let err = policy
                    .check_indexed(&table, 1, e.index, function, DEFAULT_SW_ID, [0; 16])
                    .unwrap_err();
                assert!(
                    matches!(err, PolicyError::HardDenied { .. }),
                    "{} fn {function}: {err:?}",
                    e.id
                );
            }
        }
    }
    assert_eq!(hidden, 16, "indexes 13..=28 are hidden types");
    // Fixture types 0x40, 0x60, 0x68, 0x70 all present.
    for t in [0x40u8, 0x60, 0x68, 0x70] {
        assert!(table.entries().iter().any(|e| e.feature_type == t));
    }
}

#[test]
fn index_zero_is_root_only() {
    let reply = |id| FeatureIdReply {
        id: FeatureId(id),
        feature_type: 0,
        version: 0,
    };
    let mut t = FeatureTable::new();
    // A non-Root feature at index 0 is rejected (Root "not found" returns 0).
    assert_eq!(
        t.insert(0, reply(0x2201)),
        Err(FeatureTableError::RootIndexMismatch {
            index: 0,
            id: FeatureId::ADJUSTABLE_DPI
        })
    );
    assert!(
        t.insert_entry(FeatureEntry {
            index: 0,
            id: FeatureId(0x8100),
            feature_type: 0,
            version: 0,
        })
        .is_err()
    );
    // Root at a non-zero index is rejected too.
    assert!(t.insert(4, reply(0x0000)).is_err());
    assert!(t.is_empty());
    // Root at index 0 is fine.
    t.insert(0, reply(0x0000)).unwrap();
    assert_eq!(t.len(), 1);
}
