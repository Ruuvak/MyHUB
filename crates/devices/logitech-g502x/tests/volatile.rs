//! Volatile settings (0x2201 fn3, 0x8060 fn2) over the mock link: the session
//! gate (`request_volatile`) and the driver calls built on it. No hardware.

mod common;

use common::*;
use hidpp::{FeatureId, PolicyError};
use logitech_g502x::{WriteError, onboard_mode, set_dpi, set_onboard_mode, set_report_rate};
use myhub_core::OnboardMode;
use myhub_transport::{ErrorFamily, HidppSession, SessionError};

const IDX_DPI: u8 = 7;
const IDX_RATE: u8 = 11;

/// 0x8100 fn2 IsOnboardMode, reply byte 0 = 2 (0x02 is host, 0x01 onboard).
fn mode_reply(mode: u8) -> Vec<u8> {
    long(&[0x11, 0x01, IDX_ONBOARD, 0x2b, mode])
}

fn mode_tx() -> Vec<u8> {
    hex(SYNTH_FN2_TX)
}

fn dpi_list_tx() -> Vec<u8> {
    long(&[0x11, 0x01, IDX_DPI, 0x1b, 0x00])
}

fn dpi_set_tx(dpi: u16) -> Vec<u8> {
    let [hi, lo] = dpi.to_be_bytes();
    long(&[0x11, 0x01, IDX_DPI, 0x3b, 0x00, hi, lo, 0x00])
}

fn dpi_get_tx() -> Vec<u8> {
    long(&[0x11, 0x01, IDX_DPI, 0x2b, 0x00])
}

fn dpi_get_reply(dpi: u16) -> Vec<u8> {
    let [hi, lo] = dpi.to_be_bytes();
    long(&[0x11, 0x01, IDX_DPI, 0x2b, 0x00, hi, lo, 0x06, 0x40])
}

fn rate_list_tx() -> Vec<u8> {
    long(&[0x11, 0x01, IDX_RATE, 0x0b])
}

fn rate_set_tx(interval_ms: u8) -> Vec<u8> {
    long(&[0x11, 0x01, IDX_RATE, 0x2b, interval_ms])
}

fn rate_get_tx() -> Vec<u8> {
    long(&[0x11, 0x01, IDX_RATE, 0x1b])
}

fn rate_get_reply(interval_ms: u8) -> Vec<u8> {
    long(&[0x11, 0x01, IDX_RATE, 0x1b, interval_ms])
}

/// Discovered session whose device reports host mode. Returns the number of
/// frames written so far, so tests can look at what came after discovery.
fn host_session() -> (HidppSession<MockLink>, Handle, usize) {
    let (session, handle) = discovered_session();
    handle
        .borrow_mut()
        .set_reply(mode_tx(), vec![mode_reply(2)]);
    let base = handle.borrow().writes.len();
    (session, handle, base)
}

fn writes_after(handle: &Handle, base: usize) -> Vec<Vec<u8>> {
    handle.borrow().writes[base..].to_vec()
}

/// A SetSensorDPI / SetReportRate frame on the wire: function nibble 3 of
/// 0x2201 or 2 of 0x8060.
fn is_set_frame(frame: &[u8]) -> bool {
    (frame[2] == IDX_DPI && frame[3] >> 4 == 3) || (frame[2] == IDX_RATE && frame[3] >> 4 == 2)
}

// ---------------------------------------------------------------- the gate

#[test]
fn request_volatile_passes_exactly_the_two_setters() {
    let (mut session, handle, base) = host_session();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(dpi_set_tx(1200), vec![dpi_set_tx(1200)]);
        state.set_reply(rate_set_tx(2), vec![rate_set_tx(2)]);
    }
    let mut params = [0u8; 16];
    params[1..3].copy_from_slice(&1200u16.to_be_bytes());
    session
        .request_volatile(FeatureId::ADJUSTABLE_DPI, 3, params)
        .expect("2201 fn3");
    let mut params = [0u8; 16];
    params[0] = 2;
    session
        .request_volatile(FeatureId::REPORT_RATE, 2, params)
        .expect("8060 fn2");
    assert_eq!(
        writes_after(&handle, base),
        vec![dpi_set_tx(1200), rate_set_tx(2)]
    );
}

#[test]
fn request_volatile_refuses_everything_else_without_writing() {
    let (mut session, handle, base) = host_session();
    // Allowed by the read or volatile tiers, by (feature id, function).
    let allowed = |id: u16, f: u8| {
        matches!(
            (id, f),
            (0x0001, 0 | 1)
                | (0x2201, 0..=3)
                | (0x8060, 0..=2)
                | (0x1004, 1)
                | (0x8100, 0 | 2 | 4 | 5)
        )
    };
    let entries = session.table().entries().to_vec();
    let mut refused = 0;
    for entry in entries {
        for function in 0..=15u8 {
            if allowed(entry.id.0, function) {
                continue;
            }
            let err = session
                .request_volatile(entry.id, function, [0; 16])
                .expect_err("must be refused");
            assert!(
                matches!(err, SessionError::Policy(_)),
                "{} fn {function}: {err:?}",
                entry.id
            );
            refused += 1;
        }
    }
    assert!(refused > 300, "sweep too small: {refused}");
    assert!(writes_after(&handle, base).is_empty(), "something was sent");
}

#[test]
fn request_volatile_keeps_the_onboard_write_functions_and_hidden_features_denied() {
    let (mut session, handle, base) = host_session();
    // SetActiveProfile, macros (fn 1, SetOnboardMode, is volatile).
    for function in [9u8, 10] {
        let err = session
            .request_volatile(FeatureId::ONBOARD_PROFILES, function, [0; 16])
            .unwrap_err();
        assert!(
            matches!(err, SessionError::Policy(PolicyError::HardDenied { .. })),
            "8100 fn {function}: {err:?}"
        );
    }
    // BeginWrite, WriteData, EndWrite are the onboard write tier, not volatile.
    for function in [3u8, 6, 7, 8] {
        let err = session
            .request_volatile(FeatureId::ONBOARD_PROFILES, function, [0; 16])
            .unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Policy(PolicyError::NotAllowlisted { .. })
            ),
            "8100 fn {function}: {err:?}"
        );
    }
    // Hidden / engineering rows of the fixture table (type 0x40, 0x60, 0x70).
    for id in [0x1802u16, 0x1805, 0x1890, 0x1e00, 0x1e22, 0x18c0] {
        for function in 0..=15u8 {
            let err = session
                .request_volatile(FeatureId(id), function, [0; 16])
                .unwrap_err();
            assert!(
                matches!(err, SessionError::Policy(PolicyError::HardDenied { .. })),
                "{id:#06x} fn {function}: {err:?}"
            );
        }
    }
    assert!(writes_after(&handle, base).is_empty());
}

#[test]
fn request_volatile_refuses_bad_parameters_without_writing() {
    let (mut session, handle, base) = host_session();
    let dpi = |bytes: &[(usize, u8)]| {
        let mut p = [0u8; 16];
        for &(i, v) in bytes {
            p[i] = v;
        }
        p
    };
    let cases: Vec<(FeatureId, u8, [u8; 16])> = vec![
        // DPI 0.
        (FeatureId::ADJUSTABLE_DPI, 3, dpi(&[])),
        // Junk in the unused bytes.
        (
            FeatureId::ADJUSTABLE_DPI,
            3,
            dpi(&[(1, 0x04), (2, 0xb0), (4, 1)]),
        ),
        (
            FeatureId::ADJUSTABLE_DPI,
            3,
            dpi(&[(1, 0x04), (2, 0xb0), (15, 1)]),
        ),
        // SetOnboardMode: mode byte must be 1 or 2, rest zero.
        (FeatureId::ONBOARD_PROFILES, 1, dpi(&[])),
        (FeatureId::ONBOARD_PROFILES, 1, dpi(&[(0, 3)])),
        (FeatureId::ONBOARD_PROFILES, 1, dpi(&[(0, 2), (1, 1)])),
        // Interval outside 1..=8, junk after it.
        (FeatureId::REPORT_RATE, 2, dpi(&[])),
        (FeatureId::REPORT_RATE, 2, dpi(&[(0, 9)])),
        (FeatureId::REPORT_RATE, 2, dpi(&[(0, 0xff)])),
        (FeatureId::REPORT_RATE, 2, dpi(&[(0, 2), (1, 1)])),
    ];
    for (feature, function, params) in cases {
        let err = session
            .request_volatile(feature, function, params)
            .unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Policy(PolicyError::ParamsNotAllowed { .. })
            ),
            "{feature} fn {function} {params:02x?}: {err:?}"
        );
    }
    assert!(writes_after(&handle, base).is_empty());
}

#[test]
fn request_volatile_needs_discovery_and_a_present_feature() {
    let (mut session, handle) = fixture_session();
    let err = session
        .request_volatile(FeatureId::REPORT_RATE, 2, [0; 16])
        .unwrap_err();
    assert_eq!(err, SessionError::NeedsRediscovery);
    assert!(handle.borrow().writes.is_empty());

    session.discover().expect("discover");
    let base = handle.borrow().writes.len();
    // A DFU feature the device does not list.
    let err = session
        .request_volatile(FeatureId(0x00C0), 0, [0; 16])
        .unwrap_err();
    assert_eq!(err, SessionError::FeatureAbsent(FeatureId(0x00C0)));
    assert!(writes_after(&handle, base).is_empty());
}

#[test]
fn the_plain_session_still_refuses_the_setters() {
    let (mut session, handle, base) = host_session();
    let mut params = [0u8; 16];
    params[1..3].copy_from_slice(&1200u16.to_be_bytes());
    let err = session
        .request_feature(FeatureId::ADJUSTABLE_DPI, 3, params)
        .unwrap_err();
    assert!(matches!(
        err,
        SessionError::Policy(PolicyError::NotAllowlisted { .. })
    ));
    let mut params = [0u8; 16];
    params[0] = 2;
    let err = session
        .request_feature(FeatureId::REPORT_RATE, 2, params)
        .unwrap_err();
    assert!(matches!(
        err,
        SessionError::Policy(PolicyError::NotAllowlisted { .. })
    ));
    let err = session
        .request(IDX_DPI, 3, params)
        .expect_err("raw index path is read-only too");
    assert!(matches!(err, SessionError::Policy(_)));
    assert!(writes_after(&handle, base).is_empty());
}

// ------------------------------------------------------------------ set_dpi

#[test]
fn set_dpi_sends_exactly_these_four_frames() {
    let (mut session, handle, base) = host_session();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(dpi_set_tx(1200), vec![dpi_set_tx(1200)]);
        state.set_reply(dpi_get_tx(), vec![dpi_get_reply(1200)]);
    }
    assert_eq!(set_dpi(&mut session, 1200), Ok(1200));
    let writes = writes_after(&handle, base);
    assert_eq!(
        writes.iter().map(|w| to_hex(w)).collect::<Vec<_>>(),
        [
            // 0x8100 fn2 IsOnboardMode
            "1101092b00000000000000000000000000000000",
            // 0x2201 fn1 GetSensorDPIList, sensor 0
            "1101071b00000000000000000000000000000000",
            // 0x2201 fn3 SetSensorDPI: sensor 0, dpi 0x04b0, dpiLevel 0
            "1101073b0004b000000000000000000000000000",
            // 0x2201 fn2 GetSensorDPI, sensor 0
            "1101072b00000000000000000000000000000000",
        ]
    );
}

#[test]
fn set_dpi_accepts_the_range_ends_and_steps() {
    for dpi in [100u16, 150, 25_600] {
        let (mut session, handle, _) = host_session();
        {
            let mut state = handle.borrow_mut();
            state.set_reply(dpi_set_tx(dpi), vec![dpi_set_tx(dpi)]);
            state.set_reply(dpi_get_tx(), vec![dpi_get_reply(dpi)]);
        }
        assert_eq!(set_dpi(&mut session, dpi), Ok(dpi));
    }
}

#[test]
fn set_dpi_refuses_values_outside_the_list_before_any_set_frame() {
    // Range 100..=25600 step 50.
    for dpi in [0u16, 50, 99, 101, 1201, 25_650, 26_000, u16::MAX] {
        let (mut session, handle, base) = host_session();
        assert_eq!(
            set_dpi(&mut session, dpi),
            Err(WriteError::DpiOutOfRange(dpi))
        );
        let writes = writes_after(&handle, base);
        assert_eq!(writes, vec![mode_tx(), dpi_list_tx()], "dpi {dpi}");
        assert!(!writes.iter().any(|w| is_set_frame(w)));
    }
}

#[test]
fn set_dpi_in_onboard_mode_writes_nothing_but_the_mode_read() {
    let (mut session, handle) = discovered_session();
    // The shared fixture answers 0x8100 fn2 with 01 (onboard).
    let base = handle.borrow().writes.len();
    assert_eq!(set_dpi(&mut session, 1200), Err(WriteError::NotHostMode));
    assert_eq!(writes_after(&handle, base), vec![mode_tx()]);

    // Any value other than 2 counts as not host mode.
    for mode in [0u8, 3, 0xff, 1] {
        handle
            .borrow_mut()
            .set_reply(mode_tx(), vec![mode_reply(mode)]);
        let base = handle.borrow().writes.len();
        assert_eq!(set_dpi(&mut session, 1200), Err(WriteError::NotHostMode));
        assert_eq!(writes_after(&handle, base), vec![mode_tx()]);
    }
}

#[test]
fn set_dpi_reports_a_failed_verification() {
    let (mut session, handle, _) = host_session();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(dpi_set_tx(1200), vec![dpi_set_tx(1200)]);
        // The device keeps 1600 (fixture value).
        state.set_reply(dpi_get_tx(), vec![dpi_get_reply(1600)]);
    }
    assert_eq!(
        set_dpi(&mut session, 1200),
        Err(WriteError::VerifyFailed {
            expected: 1200,
            got: 1600
        })
    );
}

#[test]
fn set_dpi_maps_a_device_error_on_the_set_frame() {
    let (mut session, handle, base) = host_session();
    handle.borrow_mut().set_reply(
        dpi_set_tx(1200),
        vec![long(&[0x11, 0x01, 0xff, IDX_DPI, 0x3b, 0x03])],
    );
    let err = set_dpi(&mut session, 1200).unwrap_err();
    assert_eq!(
        err,
        WriteError::Session(SessionError::Device {
            code: 3,
            family: ErrorFamily::Hidpp20,
            name: Some("OutOfRange"),
        })
    );
    // The read back is not attempted after a failed set.
    assert_eq!(
        writes_after(&handle, base),
        vec![mode_tx(), dpi_list_tx(), dpi_set_tx(1200)]
    );
}

#[test]
fn set_dpi_stops_when_the_mode_read_fails() {
    let (mut session, handle, base) = host_session();
    handle.borrow_mut().set_reply(
        mode_tx(),
        vec![long(&[0x11, 0x01, 0xff, IDX_ONBOARD, 0x2b, 0x09])],
    );
    let err = set_dpi(&mut session, 1200).unwrap_err();
    assert!(matches!(
        err,
        WriteError::Session(SessionError::Device { code: 9, .. })
    ));
    assert_eq!(writes_after(&handle, base), vec![mode_tx()]);
}

// -------------------------------------------------------- set_report_rate

#[test]
fn set_report_rate_sends_exactly_these_four_frames() {
    let (mut session, handle, base) = host_session();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(rate_set_tx(2), vec![rate_set_tx(2)]);
        state.set_reply(rate_get_tx(), vec![rate_get_reply(2)]);
    }
    assert_eq!(set_report_rate(&mut session, 500), Ok(500));
    let writes = writes_after(&handle, base);
    assert_eq!(
        writes.iter().map(|w| to_hex(w)).collect::<Vec<_>>(),
        [
            "1101092b00000000000000000000000000000000",
            // 0x8060 fn0 GetReportRateList
            "11010b0b00000000000000000000000000000000",
            // 0x8060 fn2 SetReportRate: 2 ms
            "11010b2b02000000000000000000000000000000",
            // 0x8060 fn1 GetReportRate
            "11010b1b00000000000000000000000000000000",
        ]
    );
}

#[test]
fn set_report_rate_accepts_every_listed_rate() {
    // Fixture list 0x8b: 1, 2, 4 and 8 ms.
    for (hz, ms) in [(1000u16, 1u8), (500, 2), (250, 4), (125, 8)] {
        let (mut session, handle, _) = host_session();
        {
            let mut state = handle.borrow_mut();
            state.set_reply(rate_set_tx(ms), vec![rate_set_tx(ms)]);
            state.set_reply(rate_get_tx(), vec![rate_get_reply(ms)]);
        }
        assert_eq!(set_report_rate(&mut session, hz), Ok(hz));
    }
}

#[test]
fn set_report_rate_refuses_unlisted_rates_before_any_set_frame() {
    for hz in [0u16, 1, 100, 333, 666, 2000, 8000, u16::MAX] {
        let (mut session, handle, base) = host_session();
        assert_eq!(
            set_report_rate(&mut session, hz),
            Err(WriteError::RateOutOfRange(hz))
        );
        let writes = writes_after(&handle, base);
        assert_eq!(writes, vec![mode_tx(), rate_list_tx()], "hz {hz}");
        assert!(!writes.iter().any(|w| is_set_frame(w)));
    }
}

#[test]
fn set_report_rate_in_onboard_mode_writes_only_the_mode_read() {
    let (mut session, handle) = discovered_session();
    let base = handle.borrow().writes.len();
    assert_eq!(
        set_report_rate(&mut session, 500),
        Err(WriteError::NotHostMode)
    );
    assert_eq!(writes_after(&handle, base), vec![mode_tx()]);
}

#[test]
fn set_report_rate_reports_a_failed_verification() {
    let (mut session, handle, _) = host_session();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(rate_set_tx(2), vec![rate_set_tx(2)]);
        // The fixture device stays at 1 ms (1000 Hz).
        state.set_reply(rate_get_tx(), vec![rate_get_reply(1)]);
    }
    assert_eq!(
        set_report_rate(&mut session, 500),
        Err(WriteError::VerifyFailed {
            expected: 500,
            got: 1000
        })
    );
}

#[test]
fn set_report_rate_maps_a_device_error_on_the_set_frame() {
    let (mut session, handle, _) = host_session();
    handle.borrow_mut().set_reply(
        rate_set_tx(2),
        vec![long(&[0x11, 0x01, 0xff, IDX_RATE, 0x2b, 0x02])],
    );
    let err = set_report_rate(&mut session, 500).unwrap_err();
    assert!(matches!(
        err,
        WriteError::Session(SessionError::Device {
            code: 2,
            family: ErrorFamily::Hidpp20,
            ..
        })
    ));
}

// ------------------------------------------------------- set_onboard_mode

fn mode_set_tx(mode: u8) -> Vec<u8> {
    long(&[0x11, 0x01, IDX_ONBOARD, 0x1b, mode])
}

#[test]
fn set_onboard_mode_host_sends_exactly_two_frames() {
    // Fixture device starts in onboard mode (reply 01).
    let (mut session, handle) = discovered_session();
    let base = handle.borrow().writes.len();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(mode_set_tx(2), vec![mode_set_tx(2)]);
        state.set_reply(mode_tx(), vec![mode_reply(2)]);
    }
    assert_eq!(
        set_onboard_mode(&mut session, OnboardMode::Host),
        Ok(OnboardMode::Host)
    );
    assert_eq!(
        writes_after(&handle, base)
            .iter()
            .map(|w| to_hex(w))
            .collect::<Vec<_>>(),
        [
            // 0x8100 fn1 SetOnboardMode: 02 = host
            "1101091b02000000000000000000000000000000",
            // 0x8100 fn2 IsOnboardMode (read back)
            "1101092b00000000000000000000000000000000",
        ]
    );
}

#[test]
fn set_onboard_mode_onboard_sends_exactly_two_frames() {
    let (mut session, handle, base) = host_session();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(mode_set_tx(1), vec![mode_set_tx(1)]);
        state.set_reply(mode_tx(), vec![mode_reply(1)]);
    }
    assert_eq!(
        set_onboard_mode(&mut session, OnboardMode::Onboard),
        Ok(OnboardMode::Onboard)
    );
    assert_eq!(
        writes_after(&handle, base)
            .iter()
            .map(|w| to_hex(w))
            .collect::<Vec<_>>(),
        [
            "1101091b01000000000000000000000000000000",
            "1101092b00000000000000000000000000000000",
        ]
    );
}

#[test]
fn set_onboard_mode_reports_a_failed_verification_and_errors() {
    let (mut session, handle, _) = host_session();
    {
        let mut state = handle.borrow_mut();
        state.set_reply(mode_set_tx(1), vec![mode_set_tx(1)]);
        // The device stays in host mode.
        state.set_reply(mode_tx(), vec![mode_reply(2)]);
    }
    assert_eq!(
        set_onboard_mode(&mut session, OnboardMode::Onboard),
        Err(WriteError::VerifyFailed {
            expected: 1,
            got: 2
        })
    );

    handle.borrow_mut().set_reply(
        mode_set_tx(2),
        vec![long(&[0x11, 0x01, 0xff, IDX_ONBOARD, 0x1b, 0x02])],
    );
    let err = set_onboard_mode(&mut session, OnboardMode::Host).unwrap_err();
    assert!(matches!(
        err,
        WriteError::Session(SessionError::Device { code: 2, .. })
    ));
}

#[test]
fn onboard_mode_reads_one_two_and_unknown() {
    let (mut session, handle, _) = host_session();
    for (byte, expected) in [
        (1u8, Some(OnboardMode::Onboard)),
        (2, Some(OnboardMode::Host)),
        (0, None),
        (3, None),
    ] {
        handle
            .borrow_mut()
            .set_reply(mode_tx(), vec![mode_reply(byte)]);
        assert_eq!(onboard_mode(&mut session), Ok(expected));
    }
}

// ------------------------------------------------------------ frame trace

static TRACE_LOG: std::sync::Mutex<Vec<(myhub_transport::FrameDirection, Vec<u8>)>> =
    std::sync::Mutex::new(Vec::new());

fn record(direction: myhub_transport::FrameDirection, frame: &[u8]) {
    TRACE_LOG.lock().unwrap().push((direction, frame.to_vec()));
}

#[test]
fn the_frame_trace_sees_the_frames_and_changes_nothing() {
    use myhub_transport::FrameDirection::{Rx, Tx};
    let (mut session, handle, base) = host_session();
    session.set_frame_trace(record);
    TRACE_LOG.lock().unwrap().clear();
    let payload = session
        .request_feature(FeatureId::ONBOARD_PROFILES, 2, [0; 16])
        .expect("reply");
    assert_eq!(payload[0], 2);
    let log = TRACE_LOG.lock().unwrap().clone();
    assert_eq!(log, vec![(Tx, mode_tx()), (Rx, mode_reply(2))]);
    assert_eq!(writes_after(&handle, base), vec![mode_tx()]);
}
