//! Policy tests through the public gate, `Policy::check_indexed`. The
//! exhaustive sweep of the internal (id, type, function) rules is a unit test
//! inside `src/policy.rs`.

mod common;

use common::{frames, table_from_json};
use hidpp::CheckedRequest;
use hidpp::feature::{FeatureEntry, FeatureId, FeatureTable};
use hidpp::frame::{DEFAULT_SW_ID, Message, decode};
use hidpp::policy::{DenyReason, Policy, PolicyError};

const READ: &[(FeatureId, &[u8])] = &[
    (FeatureId::ROOT, &[0]),
    (FeatureId::FEATURE_SET, &[0, 1]),
    (FeatureId::ADJUSTABLE_DPI, &[0, 1, 2]),
    (FeatureId::REPORT_RATE, &[0, 1]),
    (FeatureId::UNIFIED_BATTERY, &[1]),
    (FeatureId::ONBOARD_PROFILES, &[0, 2, 4, 5]),
];

const VOLATILE: &[(FeatureId, u8)] = &[
    (FeatureId::ADJUSTABLE_DPI, 3),
    (FeatureId::REPORT_RATE, 2),
    (FeatureId::ONBOARD_PROFILES, 1),
];

const IDX: u8 = 5;

fn both() -> [Policy; 2] {
    [Policy::new(), Policy::new().with_volatile_writes()]
}

/// Parameters that satisfy the volatile bounds (and are harmless for reads).
fn valid_params(id: FeatureId, f: u8) -> [u8; 16] {
    let mut p = [0u8; 16];
    match (id, f) {
        (FeatureId::REPORT_RATE, 2) => p[0] = 1,
        (FeatureId::ONBOARD_PROFILES, 1) => p[0] = 2,
        (FeatureId::ADJUSTABLE_DPI, 3) => {
            p[1] = 0x06;
            p[2] = 0x40;
        }
        _ => {}
    }
    p
}

/// Runs the gate for a one-row table holding `(id, ty)` at index 5 (Root is
/// addressed at index 0 with type 0, as the gate does not read a Root row).
fn gate(
    policy: &Policy,
    id: FeatureId,
    ty: u8,
    f: u8,
    params: [u8; 16],
) -> Result<CheckedRequest, PolicyError> {
    let mut table = FeatureTable::new();
    let idx = if id == FeatureId::ROOT {
        0
    } else {
        table
            .insert_entry(FeatureEntry {
                index: IDX,
                id,
                feature_type: ty,
                version: 0,
            })
            .unwrap();
        IDX
    };
    policy.check_indexed(&table, 1, idx, f, DEFAULT_SW_ID, params)
}

fn gate_ok(policy: &Policy, id: FeatureId, f: u8) -> Result<(), PolicyError> {
    gate(policy, id, 0, f, valid_params(id, f)).map(|_| ())
}

#[test]
fn default_is_read_only() {
    assert_eq!(Policy::default(), Policy::new());
    assert!(!Policy::new().volatile_writes_enabled());
    assert!(
        Policy::new()
            .with_volatile_writes()
            .volatile_writes_enabled()
    );
}

#[test]
fn every_read_pair_is_allowed_in_both_modes() {
    for policy in both() {
        for (id, fns) in READ {
            for &f in *fns {
                assert_eq!(gate_ok(&policy, *id, f), Ok(()), "{id} fn {f}");
            }
        }
    }
}

#[test]
fn volatile_pairs_need_explicit_enable() {
    for &(id, f) in VOLATILE {
        assert_eq!(
            gate_ok(&Policy::new(), id, f),
            Err(PolicyError::NotAllowlisted {
                feature_id: id,
                function: f
            }),
            "{id} fn {f} must be denied by default"
        );
        assert_eq!(
            gate_ok(&Policy::new().with_volatile_writes(), id, f),
            Ok(()),
            "{id} fn {f}"
        );
    }
}

#[test]
fn exact_matrix_for_known_features() {
    for policy in both() {
        let vol = policy.volatile_writes_enabled();
        for (id, _) in READ {
            for f in 0..=15u8 {
                let in_read = READ.iter().any(|(rid, fns)| rid == id && fns.contains(&f));
                let in_vol = vol && VOLATILE.contains(&(*id, f));
                let res = gate_ok(&policy, *id, f);
                assert_eq!(res.is_ok(), in_read || in_vol, "{id} fn {f} vol={vol}");
                if let Err(e) = res {
                    if *id == FeatureId::ONBOARD_PROFILES && !matches!(f, 1 | 3 | 6 | 7 | 8) {
                        assert!(matches!(e, PolicyError::HardDenied { .. }));
                    } else {
                        assert!(matches!(e, PolicyError::NotAllowlisted { .. }));
                    }
                }
            }
        }
    }
}

#[test]
fn onboard_functions_other_than_0_to_2_4_to_8_are_hard_denied_in_both_modes() {
    for policy in both() {
        for f in 0..=15u8 {
            let res = gate(&policy, FeatureId::ONBOARD_PROFILES, 0, f, [0; 16]).map(|_| ());
            if matches!(f, 0 | 2 | 4 | 5) {
                assert_eq!(res, Ok(()));
            } else if matches!(f, 3 | 6..=8) {
                // Select or write tier only: neither read nor volatile reaches it.
                assert_eq!(
                    res,
                    Err(PolicyError::NotAllowlisted {
                        feature_id: FeatureId::ONBOARD_PROFILES,
                        function: f
                    })
                );
            } else if f == 1 {
                // SetOnboardMode: volatile tier only, and never with empty
                // parameters (mode byte must be 1 or 2).
                if policy.volatile_writes_enabled() {
                    assert!(matches!(res, Err(PolicyError::ParamsNotAllowed { .. })));
                } else {
                    assert_eq!(
                        res,
                        Err(PolicyError::NotAllowlisted {
                            feature_id: FeatureId::ONBOARD_PROFILES,
                            function: 1
                        })
                    );
                }
            } else {
                assert_eq!(
                    res,
                    Err(PolicyError::HardDenied {
                        feature_id: FeatureId::ONBOARD_PROFILES,
                        function: f,
                        reason: DenyReason::OnboardWriteOrControl,
                    }),
                    "fn {f}"
                );
            }
        }
    }
}

#[test]
fn dfu_and_forbidden_ranges_are_denied_in_both_modes() {
    let mut ids: Vec<u16> = vec![0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00D0];
    ids.extend(0x1800..=0x18FF);
    ids.extend(0x1E00..=0x1EFF);
    for policy in both() {
        for &id in &ids {
            for f in 0..=15u8 {
                let err = gate(&policy, FeatureId(id), 0, f, [0; 16]).unwrap_err();
                let PolicyError::HardDenied { reason, .. } = err else {
                    panic!("{id:#06x} fn {f}: {err:?}");
                };
                if (0x00C0..=0x00D0).contains(&id) {
                    assert_eq!(reason, DenyReason::Dfu);
                } else {
                    assert_eq!(reason, DenyReason::ForbiddenFeatureId);
                }
            }
        }
    }
}

#[test]
fn dfu_neighbours_are_not_hard_denied_but_not_allowlisted() {
    for id in [0x00BFu16, 0x00C4, 0x00CF, 0x00D1] {
        assert!(matches!(
            gate(&Policy::new(), FeatureId(id), 0, 0, [0; 16]),
            Err(PolicyError::NotAllowlisted { .. })
        ));
    }
}

#[test]
fn hidden_type_bits_beat_every_tier() {
    for t in [0x20u8, 0x40, 0x60, 0x68, 0x70, 0xFF, 0x21, 0x41] {
        for policy in both() {
            for (id, fns) in READ {
                if *id == FeatureId::ROOT {
                    continue; // Root is never read from a table row.
                }
                for &f in *fns {
                    let err = gate(&policy, *id, t, f, [0; 16]).unwrap_err();
                    assert_eq!(
                        err,
                        PolicyError::HardDenied {
                            feature_id: *id,
                            function: f,
                            reason: DenyReason::HiddenFeatureType(t),
                        }
                    );
                }
            }
            for &(id, f) in VOLATILE {
                assert!(matches!(
                    gate(&policy, id, t, f, valid_params(id, f)),
                    Err(PolicyError::HardDenied { .. })
                ));
            }
        }
    }
    // Other type bits (0x80, 0x10, 0x08) are not treated as hidden.
    for t in [0x00u8, 0x08, 0x10, 0x80] {
        assert_eq!(
            gate(&Policy::new(), FeatureId::FEATURE_SET, t, 0, [0; 16]).map(|_| ()),
            Ok(())
        );
    }
}

#[test]
fn unknown_features_and_out_of_range_functions_are_denied() {
    for id in [0x1B04u16, 0x1000, 0x2202, 0x8070, 0x8110, 0x0003] {
        for policy in both() {
            for f in 0..=15u8 {
                assert!(
                    matches!(
                        gate(&policy, FeatureId(id), 0, f, [0; 16]),
                        Err(PolicyError::NotAllowlisted { .. })
                    ),
                    "{id:#06x} fn {f}"
                );
            }
        }
    }
    for policy in both() {
        for f in [16u8, 0x15, 0x50, 0xFF] {
            assert_eq!(
                gate(&policy, FeatureId::ROOT, 0, f, [0; 16]).map(|_| ()),
                Err(PolicyError::InvalidFunction(f))
            );
            assert_eq!(
                gate(&policy, FeatureId::ONBOARD_PROFILES, 0, f, [0; 16]).map(|_| ()),
                Err(PolicyError::InvalidFunction(f))
            );
        }
    }
}

#[test]
fn software_id_must_be_2_to_15() {
    let table = table_from_json();
    for policy in both() {
        for sw in [0u8, 1, 16, 0x1B, 0xFF] {
            assert_eq!(
                policy.check_indexed(&table, 1, 9, 0, sw, [0; 16]),
                Err(PolicyError::InvalidSwId(sw)),
                "sw {sw}"
            );
        }
        for sw in 2..=15u8 {
            let r = policy.check_indexed(&table, 1, 9, 0, sw, [0; 16]).unwrap();
            assert_eq!(r.sw_id(), sw);
            assert_eq!(r.encode()[3], sw);
        }
    }
}

#[test]
fn read_data_sector_bounds() {
    let p = Policy::new();
    let sector_params = |s: u16| {
        let mut params = [0u8; 16];
        params[..2].copy_from_slice(&s.to_be_bytes());
        params[2..4].copy_from_slice(&239u16.to_be_bytes());
        params
    };
    let allowed = (0x0000..=0x00FFu16).chain(0x0101..=0x0105);
    for s in allowed {
        assert_eq!(p.check_read_data_sector(s), Ok(()), "{s:#06x}");
        assert!(
            gate(&p, FeatureId::ONBOARD_PROFILES, 0, 5, sector_params(s)).is_ok(),
            "{s:#06x}"
        );
    }
    for s in [0x0100u16, 0x0106, 0x0107, 0x0200, 0x7FFF, 0xFFFE, 0xFFFF] {
        assert_eq!(
            p.check_read_data_sector(s),
            Err(PolicyError::SectorNotAllowed { sector: s })
        );
        assert_eq!(
            gate(&p, FeatureId::ONBOARD_PROFILES, 0, 5, sector_params(s)).map(|_| ()),
            Err(PolicyError::SectorNotAllowed { sector: s })
        );
    }
    // The sector bytes mean something else for other functions: not checked.
    assert!(gate(&p, FeatureId::ONBOARD_PROFILES, 0, 0, sector_params(0xFFFF)).is_ok());
}

#[test]
fn unknown_index_and_stale_table_are_denied() {
    let policy = Policy::new();
    let table = table_from_json();
    // Fixture table has indexes 1..=28; index 0 is Root.
    for idx in [29u8, 30, 0x40, 0x7F, 0xFE, 0xFF] {
        assert_eq!(
            policy.check_indexed(&table, 1, idx, 0, DEFAULT_SW_ID, [0; 16]),
            Err(PolicyError::UnknownFeatureIndex { feature_index: idx }),
            "index {idx}"
        );
    }
    // An empty table (fresh or invalidated after a 0x41 notification) knows
    // only Root.
    let empty = FeatureTable::new();
    for idx in 1..=255u8 {
        assert_eq!(
            policy.check_indexed(&empty, 1, idx, 1, DEFAULT_SW_ID, [0; 16]),
            Err(PolicyError::UnknownFeatureIndex { feature_index: idx })
        );
    }
    assert!(
        policy
            .check_indexed(&empty, 1, 0, 0, DEFAULT_SW_ID, [0; 16])
            .is_ok()
    );
    // Index 0 is Root only: Root has just function 0.
    assert_eq!(
        policy
            .check_indexed(&empty, 1, 0, 1, DEFAULT_SW_ID, [0; 16])
            .map(|_| ()),
        Err(PolicyError::NotAllowlisted {
            feature_id: FeatureId::ROOT,
            function: 1
        })
    );
    // Stale table: indexes now point at other features, so the gate decides on
    // what the table says. A table where index 9 is a DFU feature refuses it,
    // and one where 9 is hidden refuses it as hidden.
    let mut stale = FeatureTable::new();
    stale
        .insert_entry(FeatureEntry {
            index: 9,
            id: FeatureId(0x00C0),
            feature_type: 0,
            version: 0,
        })
        .unwrap();
    assert!(matches!(
        policy.check_indexed(&stale, 1, 9, 5, DEFAULT_SW_ID, [0; 16]),
        Err(PolicyError::HardDenied {
            reason: DenyReason::Dfu,
            ..
        })
    ));
    stale
        .insert_entry(FeatureEntry {
            index: 9,
            id: FeatureId::ONBOARD_PROFILES,
            feature_type: 0x70,
            version: 0,
        })
        .unwrap();
    assert!(matches!(
        policy.check_indexed(&stale, 1, 9, 5, DEFAULT_SW_ID, [0; 16]),
        Err(PolicyError::HardDenied {
            reason: DenyReason::HiddenFeatureType(0x70),
            ..
        })
    ));
}

#[test]
fn wire_index_is_bound_to_the_checked_row() {
    // The gate checks the row at the wire index, so a request for index 13
    // (0x1802, hidden type) is refused even though the function number would be
    // fine for another feature.
    let table = table_from_json();
    for policy in both() {
        for f in 0..=15u8 {
            assert!(matches!(
                policy.check_indexed(&table, 1, 13, f, DEFAULT_SW_ID, [0; 16]),
                Err(PolicyError::HardDenied { .. })
            ));
        }
    }
    // The encoded frame carries the checked index and function.
    let r = Policy::new()
        .check_indexed(&table, 1, 7, 2, DEFAULT_SW_ID, [0; 16])
        .unwrap();
    assert_eq!(&r.encode()[..4], &[0x11, 0x01, 0x07, 0x2B]);
}

#[test]
fn volatile_param_bounds() {
    let table = table_from_json();
    let p = Policy::new().with_volatile_writes();
    // Fixture: index 11 = 0x8060, index 7 = 0x2201.
    let rate = |params: [u8; 16]| p.check_indexed(&table, 1, 11, 2, DEFAULT_SW_ID, params);
    let dpi = |params: [u8; 16]| p.check_indexed(&table, 1, 7, 3, DEFAULT_SW_ID, params);
    let with = |pairs: &[(usize, u8)]| {
        let mut a = [0u8; 16];
        for &(i, v) in pairs {
            a[i] = v;
        }
        a
    };
    let denied = |r: Result<CheckedRequest, PolicyError>| {
        matches!(r, Err(PolicyError::ParamsNotAllowed { .. }))
    };

    // SetReportRate: interval 1..=8, rest zero.
    for ms in 1..=8u8 {
        assert!(rate(with(&[(0, ms)])).is_ok(), "{ms}");
    }
    for ms in [0u8, 9, 16, 255] {
        assert!(denied(rate(with(&[(0, ms)]))), "{ms}");
    }
    for i in 1..16 {
        assert!(denied(rate(with(&[(0, 1), (i, 1)]))), "byte {i}");
    }

    // SetOnboardMode (0x8100 fn1, fixture index 9): mode byte 1 or 2, rest zero.
    let mode = |params: [u8; 16]| p.check_indexed(&table, 1, 9, 1, DEFAULT_SW_ID, params);
    for ok in [1u8, 2] {
        assert!(mode(with(&[(0, ok)])).is_ok(), "{ok}");
    }
    for bad in [0u8, 3, 4, 0x80, 0xff] {
        assert!(denied(mode(with(&[(0, bad)]))), "{bad}");
    }
    for i in 1..16 {
        assert!(denied(mode(with(&[(0, 2), (i, 1)]))), "byte {i}");
    }
    // The read-only policy refuses it before looking at the parameters.
    assert_eq!(
        Policy::new()
            .check_indexed(&table, 1, 9, 1, DEFAULT_SW_ID, with(&[(0, 2)]))
            .map(|_| ()),
        Err(PolicyError::NotAllowlisted {
            feature_id: FeatureId::ONBOARD_PROFILES,
            function: 1
        })
    );
    // The neighbours stay hard denied even in the volatile tier.
    for f in [9u8, 10, 15] {
        assert!(
            matches!(
                p.check_indexed(&table, 1, 9, f, DEFAULT_SW_ID, with(&[(0, 2)])),
                Err(PolicyError::HardDenied { .. })
            ),
            "fn {f}"
        );
    }
    // The write functions are not reachable through the volatile gate.
    for f in [3u8, 6, 7, 8] {
        assert!(
            matches!(
                p.check_indexed(&table, 1, 9, f, DEFAULT_SW_ID, with(&[(0, 2)])),
                Err(PolicyError::NotAllowlisted { .. })
            ),
            "fn {f}"
        );
    }

    // SetSensorDPI: sensor 0, non-zero DPI at 1..3, level byte 3 pinned to 0, 4.. zero.
    assert!(dpi(with(&[(1, 0x06), (2, 0x40)])).is_ok());
    assert!(dpi(with(&[(1, 0x00), (2, 0x01)])).is_ok());
    assert!(dpi(with(&[(1, 0x01), (2, 0x00)])).is_ok());
    assert!(denied(dpi(with(&[]))));
    for sensor in 1..=255u8 {
        assert!(
            denied(dpi(with(&[(0, sensor), (1, 0x06), (2, 0x40)]))),
            "sensor {sensor}"
        );
    }
    for level in 1..=255u8 {
        assert!(
            denied(dpi(with(&[(1, 0x06), (2, 0x40), (3, level)]))),
            "level {level}"
        );
    }
    for i in 4..16 {
        assert!(
            denied(dpi(with(&[(1, 0x06), (2, 0x40), (i, 1)]))),
            "byte {i}"
        );
    }

    // Bounds never apply to (or relax) other calls, and the default policy
    // refuses the setters before looking at parameters.
    assert_eq!(
        Policy::new()
            .check_indexed(&table, 1, 11, 2, DEFAULT_SW_ID, with(&[(0, 1)]))
            .map(|_| ()),
        Err(PolicyError::NotAllowlisted {
            feature_id: FeatureId::REPORT_RATE,
            function: 2
        })
    );
    assert!(
        p.check_indexed(&table, 1, 11, 1, DEFAULT_SW_ID, with(&[(0, 200), (5, 9)]))
            .is_ok()
    );
}

#[test]
fn all_logged_tx_frames_pass_the_gate_and_reencode_identically() {
    let table = table_from_json();
    let policy = Policy::new();
    let mut n = 0;
    for (is_tx, raw) in frames() {
        if !is_tx {
            continue;
        }
        let Message::Response {
            device_index,
            feature_index,
            function,
            sw_id,
            payload,
        } = decode(&raw).unwrap()
        else {
            panic!("unexpected frame shape");
        };
        let req = policy
            .check_indexed(
                &table,
                device_index,
                feature_index,
                function,
                sw_id,
                payload,
            )
            .unwrap_or_else(|e| panic!("{raw:02x?}: {e}"));
        assert_eq!(req.encode().as_slice(), raw.as_slice());
        n += 1;
    }
    assert_eq!(n, 165);
}

#[test]
fn error_messages_mention_feature_and_function() {
    let e = gate(&Policy::new(), FeatureId::ONBOARD_PROFILES, 0, 6, [0; 16])
        .map(|_| ())
        .unwrap_err();
    let s = e.to_string();
    assert!(s.contains("0x8100") && s.contains('6'), "{s}");
}

fn begin_params(sector: u16, offset: u16, length: u16) -> [u8; 16] {
    let mut p = [0u8; 16];
    p[..2].copy_from_slice(&sector.to_be_bytes());
    p[2..4].copy_from_slice(&offset.to_be_bytes());
    p[4..6].copy_from_slice(&length.to_be_bytes());
    p
}

#[test]
fn onboard_write_gate_is_separate_and_off_by_default() {
    assert!(!Policy::new().onboard_writes_enabled());
    assert!(
        !Policy::new()
            .with_volatile_writes()
            .onboard_writes_enabled()
    );
    let w = Policy::new().with_onboard_writes();
    assert!(w.onboard_writes_enabled());
    assert!(!w.volatile_writes_enabled());
    // Volatile pairs stay denied behind the write gate alone.
    for &(id, f) in VOLATILE {
        assert!(matches!(
            gate(&w, id, 0, f, valid_params(id, f)),
            Err(PolicyError::NotAllowlisted { .. })
        ));
    }
}

#[test]
fn write_functions_are_denied_without_the_gate() {
    for policy in both() {
        for (f, params) in [
            (6u8, begin_params(3, 0, 255)),
            (7, [0xAA; 16]),
            (8, [0; 16]),
        ] {
            assert_eq!(
                gate(&policy, FeatureId::ONBOARD_PROFILES, 0, f, params).map(|_| ()),
                Err(PolicyError::NotAllowlisted {
                    feature_id: FeatureId::ONBOARD_PROFILES,
                    function: f
                })
            );
        }
    }
}

#[test]
fn write_tier_accepts_allowlisted_sectors_only() {
    let w = Policy::new().with_onboard_writes();
    let begin = |p| gate(&w, FeatureId::ONBOARD_PROFILES, 0, 6, p).map(|_| ());
    for s in [0x0000u16, 0x0003, 0x0004, 0x0005] {
        assert_eq!(begin(begin_params(s, 0, 255)), Ok(()), "{s:#06x}");
    }
    for s in [0x0001u16, 0x0002, 0x0006, 0x0100, 0x0106, 0x00FF, 0xFFFF] {
        assert_eq!(
            begin(begin_params(s, 0, 255)),
            Err(PolicyError::HardDenied {
                feature_id: FeatureId::ONBOARD_PROFILES,
                function: 6,
                reason: DenyReason::SectorNotWritable(s),
            }),
            "{s:#06x}"
        );
    }
    for s in 0x0101..=0x0105u16 {
        assert_eq!(
            begin(begin_params(s, 0, 255)),
            Err(PolicyError::HardDenied {
                feature_id: FeatureId::ONBOARD_PROFILES,
                function: 6,
                reason: DenyReason::RomSectorWrite(s),
            }),
            "{s:#06x}"
        );
    }
}

#[test]
fn write_tier_bounds_offset_length_and_padding() {
    let w = Policy::new().with_onboard_writes();
    let begin = |p| gate(&w, FeatureId::ONBOARD_PROFILES, 0, 6, p).map(|_| ());
    let denied =
        |r: Result<(), PolicyError>| matches!(r, Err(PolicyError::ParamsNotAllowed { .. }));
    for off in [1u16, 16, 239, 255, 0x8000] {
        assert!(denied(begin(begin_params(3, off, 255))), "offset {off}");
    }
    for len in [0u16, 1, 16, 254, 256, 0xFFFF] {
        assert!(denied(begin(begin_params(3, 0, len))), "length {len}");
    }
    for i in 6..16 {
        let mut p = begin_params(3, 0, 255);
        p[i] = 1;
        assert!(denied(begin(p)), "byte {i}");
    }
    // EndWrite takes no parameters; WriteData takes any 16 bytes.
    let end = |p| gate(&w, FeatureId::ONBOARD_PROFILES, 0, 8, p).map(|_| ());
    assert_eq!(end([0; 16]), Ok(()));
    for i in 0..16 {
        let mut p = [0u8; 16];
        p[i] = 1;
        assert!(denied(end(p)), "byte {i}");
    }
    assert!(gate(&w, FeatureId::ONBOARD_PROFILES, 0, 7, [0xFF; 16]).is_ok());
}

#[test]
fn write_tier_keeps_other_onboard_functions_denied_and_reads_unchanged() {
    let w = Policy::new().with_onboard_writes().with_volatile_writes();
    for f in [9u8, 10, 11, 12, 13, 14, 15] {
        assert_eq!(
            gate(&w, FeatureId::ONBOARD_PROFILES, 0, f, [0; 16]).map(|_| ()),
            Err(PolicyError::HardDenied {
                feature_id: FeatureId::ONBOARD_PROFILES,
                function: f,
                reason: DenyReason::OnboardWriteOrControl,
            }),
            "fn {f}"
        );
    }
    // Hidden type beats the write tier too.
    for f in [6u8, 7, 8] {
        assert!(matches!(
            gate(
                &w,
                FeatureId::ONBOARD_PROFILES,
                0x40,
                f,
                begin_params(3, 0, 255)
            ),
            Err(PolicyError::HardDenied {
                reason: DenyReason::HiddenFeatureType(0x40),
                ..
            })
        ));
    }
    // Read tier is unchanged: ReadData still reads ROM sectors.
    assert!(
        gate(
            &w,
            FeatureId::ONBOARD_PROFILES,
            0,
            5,
            begin_params(0x0101, 239, 0)
        )
        .is_ok()
    );
}

fn select_params(sector: u16) -> [u8; 16] {
    let mut p = [0u8; 16];
    p[..2].copy_from_slice(&sector.to_be_bytes());
    p
}

#[test]
fn profile_select_gate_is_separate() {
    assert!(!Policy::new().profile_select_enabled());
    assert!(
        !Policy::new()
            .with_volatile_writes()
            .profile_select_enabled()
    );
    assert!(!Policy::new().with_onboard_writes().profile_select_enabled());
    let sel = Policy::new().with_profile_select();
    assert!(sel.profile_select_enabled());
    assert!(!sel.volatile_writes_enabled() && !sel.onboard_writes_enabled());
    let ob = FeatureId::ONBOARD_PROFILES;
    let not_allowlisted = |function| {
        Err(PolicyError::NotAllowlisted {
            feature_id: ob,
            function,
        })
    };
    // fn3 is denied without its gate, also behind the other two gates.
    for policy in [
        Policy::new(),
        Policy::new().with_volatile_writes(),
        Policy::new().with_onboard_writes(),
        Policy::new().with_volatile_writes().with_onboard_writes(),
    ] {
        assert_eq!(
            gate(&policy, ob, 0, 3, select_params(3)).map(|_| ()),
            not_allowlisted(3)
        );
    }
    // The select gate alone opens nothing else.
    for f in [1u8, 6, 7, 8] {
        assert_eq!(
            gate(&sel, ob, 0, f, valid_params(ob, f)).map(|_| ()),
            not_allowlisted(f),
            "fn {f}"
        );
    }
    for f in 9..=15u8 {
        assert!(matches!(
            gate(&sel, ob, 0, f, [0; 16]),
            Err(PolicyError::HardDenied {
                reason: DenyReason::OnboardWriteOrControl,
                ..
            })
        ));
    }
    for &(id, f) in VOLATILE {
        if id != ob {
            assert_eq!(
                gate(&sel, id, 0, f, valid_params(id, f)).map(|_| ()),
                Err(PolicyError::NotAllowlisted {
                    feature_id: id,
                    function: f
                })
            );
        }
    }
}

#[test]
fn profile_select_bounds() {
    let sel = Policy::new().with_profile_select();
    let ob = FeatureId::ONBOARD_PROFILES;
    let select = |p| gate(&sel, ob, 0, 3, p).map(|_| ());
    for s in 1..=5u16 {
        assert_eq!(select(select_params(s)), Ok(()), "{s}");
    }
    for s in [0u16, 6, 0x00FF, 0x0100, 0x0106, 0x7FFF, 0xFFFF] {
        assert_eq!(
            select(select_params(s)),
            Err(PolicyError::HardDenied {
                feature_id: ob,
                function: 3,
                reason: DenyReason::SectorNotSelectable(s),
            }),
            "{s:#06x}"
        );
    }
    for s in 0x0101..=0x0105u16 {
        assert_eq!(
            select(select_params(s)),
            Err(PolicyError::HardDenied {
                feature_id: ob,
                function: 3,
                reason: DenyReason::RomSectorSelect(s),
            }),
            "{s:#06x}"
        );
    }
    for i in 2..16 {
        let mut p = select_params(3);
        p[i] = 1;
        assert!(
            matches!(select(p), Err(PolicyError::ParamsNotAllowed { .. })),
            "byte {i}"
        );
    }
    // Hidden type beats the select tier.
    assert!(matches!(
        gate(&sel, ob, 0x40, 3, select_params(3)),
        Err(PolicyError::HardDenied {
            reason: DenyReason::HiddenFeatureType(0x40),
            ..
        })
    ));
}

#[test]
fn directory_sector_is_writable_only_through_the_onboard_write_gate() {
    let ob = FeatureId::ONBOARD_PROFILES;
    let begin = |policy: &Policy| gate(policy, ob, 0, 6, begin_params(0, 0, 255)).map(|_| ());
    assert_eq!(begin(&Policy::new().with_onboard_writes()), Ok(()));
    for policy in [
        Policy::new(),
        Policy::new().with_volatile_writes(),
        Policy::new().with_profile_select(),
    ] {
        assert_eq!(
            begin(&policy),
            Err(PolicyError::NotAllowlisted {
                feature_id: ob,
                function: 6
            })
        );
    }
    // Bounds still apply to 0x0000, and the other user sectors stay denied.
    let w = Policy::new().with_onboard_writes();
    assert!(matches!(
        gate(&w, ob, 0, 6, begin_params(0, 16, 255)),
        Err(PolicyError::ParamsNotAllowed { .. })
    ));
    for s in [1u16, 2] {
        assert!(matches!(
            gate(&w, ob, 0, 6, begin_params(s, 0, 255)),
            Err(PolicyError::HardDenied {
                reason: DenyReason::SectorNotWritable(_),
                ..
            })
        ));
    }
    assert!(matches!(
        gate(&w, ob, 0, 6, begin_params(0x0101, 0, 255)),
        Err(PolicyError::HardDenied {
            reason: DenyReason::RomSectorWrite(0x0101),
            ..
        })
    ));
}

#[test]
fn button_spy_gate_is_separate_and_bounded() {
    let spy = FeatureId::MOUSE_BUTTON_SPY;
    assert_eq!(spy, FeatureId(0x8110));
    assert!(!Policy::new().button_spy_enabled());
    for p in [
        Policy::new().with_volatile_writes(),
        Policy::new().with_onboard_writes(),
        Policy::new().with_profile_select(),
        Policy::new()
            .with_volatile_writes()
            .with_onboard_writes()
            .with_profile_select(),
    ] {
        assert!(!p.button_spy_enabled());
        // Not reachable through any other gate, start or stop.
        for f in [1u8, 2] {
            assert_eq!(
                gate(&p, spy, 0, f, [0; 16]).map(|_| ()),
                Err(PolicyError::NotAllowlisted {
                    feature_id: spy,
                    function: f
                })
            );
        }
    }
    let s = Policy::new().with_button_spy();
    assert!(s.button_spy_enabled());
    assert!(!s.volatile_writes_enabled() && !s.onboard_writes_enabled());
    assert!(!s.profile_select_enabled());
    for f in [1u8, 2] {
        assert_eq!(gate(&s, spy, 0, f, [0; 16]).map(|_| ()), Ok(()));
        // Every non-zero parameter byte is refused.
        for i in 0..16 {
            let mut p = [0u8; 16];
            p[i] = 1;
            assert!(
                matches!(
                    gate(&s, spy, 0, f, p),
                    Err(PolicyError::ParamsNotAllowed { .. })
                ),
                "fn {f} byte {i}"
            );
        }
    }
    // The spy gate opens nothing else.
    let ob = FeatureId::ONBOARD_PROFILES;
    for (id, f) in [(ob, 1u8), (ob, 3), (ob, 6), (ob, 7), (ob, 8)] {
        assert!(matches!(
            gate(&s, id, 0, f, valid_params(id, f)),
            Err(PolicyError::NotAllowlisted { .. })
        ));
    }
    for &(id, f) in VOLATILE {
        if id != ob {
            assert!(gate(&s, id, 0, f, valid_params(id, f)).is_err());
        }
    }
    // Hidden type beats the spy tier.
    assert!(matches!(
        gate(&s, spy, 0x40, 1, [0; 16]),
        Err(PolicyError::HardDenied {
            reason: DenyReason::HiddenFeatureType(0x40),
            ..
        })
    ));
}

#[test]
fn button_spy_fn3_and_fn4_and_the_rest_are_denied_in_every_gate() {
    let spy = FeatureId::MOUSE_BUTTON_SPY;
    for combo in 0..16u8 {
        let mut p = Policy::new();
        if combo & 1 != 0 {
            p = p.with_volatile_writes();
        }
        if combo & 2 != 0 {
            p = p.with_onboard_writes();
        }
        if combo & 4 != 0 {
            p = p.with_profile_select();
        }
        if combo & 8 != 0 {
            p = p.with_button_spy();
        }
        for f in [0u8, 3, 4, 5, 6, 7, 8, 9, 10, 15] {
            for params in [[0u8; 16], [0xFF; 16]] {
                assert!(gate(&p, spy, 0, f, params).is_err(), "combo {combo} fn {f}");
            }
            // Denied as not allowlisted (the feature is public, type 0).
            assert!(matches!(
                gate(&p, spy, 0, f, [0; 16]),
                Err(PolicyError::NotAllowlisted { .. })
            ));
        }
    }
}
