//! `read_snapshot` over the mock link (real device frames, no hardware).

mod common;

use std::collections::BTreeSet;

use common::*;
use hidpp::crc::sector_crc_ok;
use hidpp::dpi::DpiEntry;
use hidpp::onboard::DirectoryEntry;
use logitech_g502x::{ReadError, read_snapshot};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(FIXTURE_JSON).expect("fixture json")
}

#[test]
fn snapshot_reproduces_fixture() {
    let (mut session, _handle) = fixture_session();
    let snap = read_snapshot(&mut session).expect("snapshot");
    let json = fixture();

    // Feature table: 28 rows, identical to the probe output.
    assert_eq!(snap.features.len(), 28);
    for (row, want) in snap
        .features
        .iter()
        .zip(json["features"].as_array().unwrap())
    {
        assert_eq!(u64::from(row.index), want["index"].as_u64().unwrap());
        assert_eq!(row.id, want["id"].as_str().unwrap());
        assert_eq!(u64::from(row.feature_type), want["type"].as_u64().unwrap());
        assert_eq!(u64::from(row.version), want["version"].as_u64().unwrap());
    }

    // DPI 100..25600 step 50, current and default 1600.
    assert_eq!(snap.dpi.sensor_count, 1);
    assert_eq!(
        snap.dpi.list.entries,
        vec![DpiEntry::Range {
            min: 100,
            max: 25600,
            step: 50
        }]
    );
    assert_eq!(snap.dpi.current, 1600);
    assert_eq!(snap.dpi.default, 1600);

    // Report rate.
    assert_eq!(snap.report_rate.rates_hz, vec![1000, 500, 250, 125]);
    assert_eq!(snap.report_rate.current_hz, 1000);
    assert_eq!(snap.report_rate.current_interval_ms, 1);

    // Battery.
    assert_eq!(snap.battery.percent, 55);
    assert!(snap.battery.raw_hex.starts_with("37040000"));

    // Onboard: info, synthetic mode/profile reads, directory.
    let ob = &snap.onboard;
    assert_eq!(ob.info.profile_format, 3);
    assert_eq!(ob.info.sector_size, 255);
    assert_eq!(ob.mode_raw_hex.as_deref(), Some("01000000"));
    assert_eq!(ob.mode_error, None);
    assert_eq!(ob.active_profile_raw_hex.as_deref(), Some("00010000"));
    assert_eq!(ob.active_profile_error, None);
    let expected_dir: Vec<DirectoryEntry> =
        [(1, true), (2, true), (3, false), (4, false), (5, false)]
            .into_iter()
            .map(|(sector, enabled)| DirectoryEntry { sector, enabled })
            .collect();
    assert_eq!(ob.directory.entries, expected_dir);

    // All 8 sectors byte-equal to the fixture; CRC ok on 0x0000..0x0005.
    let fixture_sectors = json["sectors"].as_object().unwrap();
    assert_eq!(fixture_sectors.len(), 8);
    assert_eq!(ob.sectors.len(), 8);
    for (key, want) in fixture_sectors {
        let got = ob
            .sectors
            .get(key)
            .unwrap_or_else(|| panic!("missing {key}"));
        assert_eq!(hex(got), hex(want.as_str().unwrap()), "sector {key}");
    }
    for sector in 0..=5u16 {
        let key = format!("0x{sector:04x}");
        assert!(sector_crc_ok(&hex(&ob.sectors[&key])), "CRC of {key}");
    }
    assert_eq!(ob.profiles_skipped, None);
    assert_eq!(ob.profiles.len(), 7);
    for p in &ob.profiles {
        assert_eq!(p.error, None, "{}", p.sector);
        let profile = p.profile.as_ref().expect("parsed");
        if !p.rom {
            assert!(profile.crc_ok, "profile CRC of {}", p.sector);
        }
    }

    // Step accounting: 30 discovery + 7 reads + 2 onboard reads + 8 * 16 chunks.
    assert_eq!(snap.total_requests, 167);
    assert_eq!(snap.steps.iter().map(|s| s.requests).sum::<u64>(), 167);
    assert_eq!(snap.steps.first().unwrap().step, "discover");
    assert_eq!(snap.steps.first().unwrap().requests, 30);
    assert_eq!(session.request_count(), 167);

    // The whole snapshot serialises (the UI and the example need that).
    let value = serde_json::to_value(&snap).expect("serialise");
    assert_eq!(value["battery"]["percent"], 55);
}

#[test]
fn only_logged_or_synthetic_frames_are_sent() {
    let (mut session, handle) = fixture_session();
    read_snapshot(&mut session).expect("snapshot");

    let allowed: BTreeSet<Vec<u8>> = logged_pairs()
        .into_iter()
        .map(|(tx, _)| tx)
        .chain([hex(SYNTH_FN2_TX), hex(SYNTH_FN4_TX)])
        .collect();
    let state = handle.borrow();
    let sent: BTreeSet<Vec<u8>> = state.writes.iter().cloned().collect();

    // Nothing outside the logged set plus the two synthetic reads went out ...
    assert_eq!(sent.difference(&allowed).count(), 0, "unlisted TX sent");
    // ... each frame went out exactly once, and every allowed frame was used.
    assert_eq!(sent.len(), state.writes.len(), "a frame was sent twice");
    assert_eq!(sent, allowed);
    assert_eq!(state.writes.len(), 167);
}

#[test]
fn unexercised_onboard_reads_fail_softly() {
    let (mut session, handle) = fixture_session();
    {
        let mut state = handle.borrow_mut();
        // fn2: HID++ 2.0 error 9 (Unsupported). fn4: no reply at all.
        state.set_reply(
            hex(SYNTH_FN2_TX),
            vec![long(&[0x11, 0x01, 0xFF, IDX_ONBOARD, 0x2b, 0x09])],
        );
        state.set_reply(hex(SYNTH_FN4_TX), vec![]);
    }
    let snap = read_snapshot(&mut session).expect("non-fatal");
    let ob = &snap.onboard;
    assert_eq!(ob.mode_raw_hex, None);
    assert!(ob.mode_error.as_deref().unwrap().contains("Unsupported"));
    assert_eq!(ob.active_profile_raw_hex, None);
    assert!(
        ob.active_profile_error
            .as_deref()
            .unwrap()
            .contains("no reply")
    );
    // The rest of the snapshot is intact.
    assert_eq!(ob.sectors.len(), 8);
}

fn patched_info_reply(handle: &Handle, patch: impl FnOnce(&mut [u8])) {
    let tx = long(&[0x11, 0x01, IDX_ONBOARD, 0x0b]);
    let mut state = handle.borrow_mut();
    let mut reply = state.responses[&tx][0].clone();
    patch(&mut reply[4..]);
    state.set_reply(tx, vec![reply]);
}

#[test]
fn other_profile_formats_keep_raw_bytes_and_skip_parsing() {
    let (mut session, handle) = fixture_session();
    patched_info_reply(&handle, |payload| payload[1] = 2);
    let snap = read_snapshot(&mut session).expect("snapshot");
    assert!(snap.onboard.profiles_skipped.is_some());
    assert!(snap.onboard.profiles.is_empty());
    assert_eq!(snap.onboard.sectors.len(), 8);
}

fn assert_no_read_data(handle: &Handle) {
    let state = handle.borrow();
    let read_data_sent = state
        .writes
        .iter()
        .any(|w| w[2] == IDX_ONBOARD && w[3] >> 4 == 5);
    assert!(!read_data_sent, "a ReadData request was sent");
}

#[track_caller]
fn assert_info_refused(patch: impl FnOnce(&mut [u8]), field: &str) {
    let (mut session, handle) = fixture_session();
    patched_info_reply(&handle, patch);
    let err = read_snapshot(&mut session).unwrap_err();
    assert!(
        matches!(&err, ReadError::InfoOutOfBounds { field: f, .. } if f.starts_with(field)),
        "unexpected error {err:?}"
    );
    assert_no_read_data(&handle);
}

#[test]
fn rom_profile_count_above_five_is_refused_before_any_read_data() {
    // 6 ROM profiles would need sector 0x0106, which the policy never allows.
    assert_info_refused(|payload| payload[4] = 6, "rom_profile_count");
}

#[test]
fn profile_count_above_sixteen_is_refused_before_any_read_data() {
    // Offset 6 is sector_count; raise it so only the 16 slot limit trips.
    assert_info_refused(
        |payload| {
            payload[3] = 17;
            payload[6] = 40;
        },
        "profile_count",
    );
}

#[test]
fn profile_count_above_sector_count_is_refused_before_any_read_data() {
    assert_info_refused(|payload| payload[6] = 3, "profile_count vs sector_count");
}

#[test]
fn snapshot_records_whether_short_reports_were_visible() {
    let (mut session, _handle) = fixture_session();
    assert!(read_snapshot(&mut session).unwrap().short_reports_available);

    let (mut session, handle) = fixture_session();
    handle.borrow_mut().short_available = false;
    let snap = read_snapshot(&mut session).unwrap();
    assert!(!snap.short_reports_available);
    let value = serde_json::to_value(&snap).unwrap();
    assert_eq!(value["short_reports_available"], false);
}
