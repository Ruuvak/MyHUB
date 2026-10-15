//! Stage 6 slice B: save to the mouse, battery status, host-mode fixes.

use std::path::PathBuf;

use hidpp::onboard_write::WriteError as PatchError;
use logitech_g502x::{FailureDetail, ReadbackCheck, SequenceLog};
use myhub_core::{BatteryLevel, ChargingState};

use super::*;

const FIXTURE: &str = include_str!("../../../../crates/hidpp/tests/fixtures/hidpp_read.json");

// ------------------------------------------------------------ the simulation

#[derive(Default)]
struct SimState {
    flash: HashMap<u16, Vec<u8>>,
    active: u16,
    mode: u8,
    pending: Option<(u16, Vec<u8>)>,
    /// `(function, params)` of every 0x8100 frame.
    calls: Vec<(u8, Vec<u8>)>,
}

type Sim = Rc<RefCell<SimState>>;

fn reply_frame(report: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut frame = report[..4].to_vec();
    frame.extend_from_slice(payload);
    frame.resize(20, 0);
    frame
}

fn hook(sim: Sim) -> OnboardHook {
    Box::new(move |report| {
        let function = report[3] >> 4;
        let params = &report[4..];
        let mut s = sim.borrow_mut();
        s.calls.push((function, params.to_vec()));
        let ack = || Some(vec![reply_frame(report, &[])]);
        match function {
            1 => {
                s.mode = params[0];
                ack()
            }
            2 => Some(vec![reply_frame(report, &[s.mode])]),
            3 => {
                s.active = u16::from_be_bytes([params[0], params[1]]);
                ack()
            }
            4 => Some(vec![reply_frame(report, &s.active.to_be_bytes())]),
            5 => {
                let sector = u16::from_be_bytes([params[0], params[1]]);
                let off = usize::from(u16::from_be_bytes([params[2], params[3]]));
                Some(vec![reply_frame(report, &s.flash[&sector][off..off + 16])])
            }
            6 => {
                let sector = u16::from_be_bytes([params[0], params[1]]);
                s.pending = Some((sector, Vec::new()));
                ack()
            }
            7 => {
                if let Some((_, buf)) = s.pending.as_mut() {
                    buf.extend_from_slice(params);
                }
                ack()
            }
            8 => {
                if let Some((sector, buf)) = s.pending.take()
                    && buf.len() >= 256
                {
                    s.flash.insert(sector, buf[..255].to_vec());
                }
                ack()
            }
            // fn0 (info) comes from the recorded frames.
            _ => None,
        }
    })
}

fn fixture_sectors() -> HashMap<u16, Vec<u8>> {
    let json: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    json["sectors"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| {
            (
                u16::from_str_radix(key.trim_start_matches("0x"), 16).unwrap(),
                hex(value.as_str().unwrap()),
            )
        })
        .collect()
}

/// A device over the simulated onboard memory, already read once.
fn save_device(active: u16, mode: u8, enable_3: bool) -> (Device<MockLink>, Handle, Sim) {
    let enabled: &[u16] = if enable_3 { &[1, 2, 3] } else { &[1, 2] };
    save_device_with(active, mode, enabled)
}

/// Like [`save_device`] with exactly these user sectors enabled in the directory.
fn save_device_with(active: u16, mode: u8, enabled: &[u16]) -> (Device<MockLink>, Handle, Sim) {
    let mut flash = fixture_sectors();
    {
        let dir = flash.get_mut(&0).unwrap();
        // Entries `{sector BE16, enabled, 0xff}` for the sectors 1..=5.
        for k in 0..5 {
            let sector = u16::from_be_bytes([dir[4 * k], dir[4 * k + 1]]);
            dir[4 * k + 2] = u8::from(enabled.contains(&sector));
        }
        let crc = hidpp::crc::sector_crc_computed(dir).unwrap();
        dir[253..255].copy_from_slice(&crc.to_be_bytes());
    }
    let sim: Sim = Rc::new(RefCell::new(SimState {
        flash,
        active,
        mode,
        ..SimState::default()
    }));
    let (mut device, handle) = mock_device();
    handle.borrow_mut().onboard_hook = Some(hook(Rc::clone(&sim)));
    step(&mut device);
    device.take_dirty();
    device.take_restore_due();
    (device, handle, sim)
}

fn calls(sim: &Sim, function: u8) -> Vec<Vec<u8>> {
    sim.borrow()
        .calls
        .iter()
        .filter(|(f, _)| *f == function)
        .map(|(_, p)| p.clone())
        .collect()
}

fn position(sim: &Sim, function: u8) -> Option<usize> {
    sim.borrow().calls.iter().position(|(f, _)| *f == function)
}

fn temp_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "myhub-s6-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn request() -> SaveRequest {
    SaveRequest {
        dpi_levels: Some(vec![400, 800, 1200, 1600, 3200]),
        default_dpi_index: Some(1),
        shift_dpi_index: None,
        report_rate_hz: Some(250),
    }
}

fn far() -> Option<Instant> {
    Some(Instant::now() + Duration::from_secs(60))
}

fn save(
    device: &mut Device<MockLink>,
    root: &Path,
    allow: bool,
) -> Result<SaveOutcome, WriteFailure> {
    device.save_profile(ID, &request(), allow, far(), Some(root), 1_760_520_605)
}

fn code<T: std::fmt::Debug>(result: Result<T, WriteFailure>) -> String {
    match result {
        Err(WriteFailure::Refused(code)) => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// --------------------------------------------------------------- the save

#[test]
fn the_active_profile_3_is_writable_in_the_detail() {
    let (device, _handle, _sim) = save_device(3, 1, true);
    assert_eq!(
        device.detail().unwrap().onboard.unwrap().writable_profile,
        Some(3)
    );
    let (device, _handle, _sim) = save_device(2, 1, true);
    assert_eq!(
        device.detail().unwrap().onboard.unwrap().writable_profile,
        None
    );
}

#[test]
fn a_save_backs_up_writes_reselects_and_rereads() {
    let (mut device, _handle, sim) = save_device(3, 1, true);
    let original = sim.borrow().flash[&3].clone();
    let root = temp_dir();
    let outcome = save(&mut device, &root, true).expect("save");
    assert!(outcome.written);

    // The backup holds the sector as it was.
    let path = outcome.backup_path.expect("backup path");
    assert!(
        path.ends_with("20251015T093005Z_sector_0003.json"),
        "{path:?}"
    );
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(value["deviceId"], ID);
    assert_eq!(value["sector"], 3);
    assert_eq!(value["crc"], "0xa498");
    assert_eq!(
        value["hex"].as_str().unwrap(),
        original
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );

    // One BeginWrite, 16 data frames, one EndWrite, then fn3 reloads the profile:
    // away to 0x0001 and back to 0x0003 (fn3 to the active sector alone does not
    // reload it).
    assert_eq!(calls(&sim, 6).len(), 1);
    assert_eq!(calls(&sim, 7).len(), 16);
    assert_eq!(calls(&sim, 8).len(), 1);
    let select = |n: u8| {
        let mut p = vec![0u8; 16];
        p[1] = n;
        p
    };
    assert_eq!(calls(&sim, 3), vec![select(1), select(3)]);
    assert!(position(&sim, 8) < position(&sim, 3));

    // The flash changed, the detail was read again and shows it.
    assert_ne!(sim.borrow().flash[&3], original);
    assert!(hidpp::crc::sector_crc_ok(&sim.borrow().flash[&3]));
    assert!(device.take_dirty());
    let detail = device.detail().unwrap();
    let profile = detail
        .onboard
        .unwrap()
        .profiles
        .into_iter()
        .find(|p| p.id == "user-3")
        .unwrap();
    assert_eq!(profile.dpi_levels, vec![400, 800, 1200, 1600, 3200]);
    assert_eq!(profile.default_dpi_index, 1);
    assert_eq!(profile.report_rate_hz, Some(250));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_identical_save_is_unchanged_but_still_has_a_backup() {
    let (mut device, _handle, sim) = save_device(3, 1, true);
    let root = temp_dir();
    assert!(save(&mut device, &root, true).unwrap().written);
    let again = save(&mut device, &root, true).expect("second save");
    assert!(!again.written);
    assert!(!again.reloaded);
    assert!(again.backup_path.as_ref().is_some_and(|p| p.exists()));
    // No second write sequence and no second reload (still the two fn3 of the
    // first save).
    assert_eq!(calls(&sim, 6).len(), 1);
    assert_eq!(calls(&sim, 3).len(), 2);
    assert_eq!(std::fs::read_dir(root.join(ID)).unwrap().count(), 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn refusals_send_no_frame() {
    let root = temp_dir();
    let (mut device, handle, sim) = save_device(3, 1, true);
    let sent = handle.borrow().sent.len();
    let before = sim.borrow().calls.len();

    assert_eq!(code(save(&mut device, &root, false)), "writesDisabled");
    assert_eq!(
        code(device.save_profile("other", &request(), true, far(), Some(&root), 0)),
        "disconnected"
    );
    let expired = Some(Instant::now());
    assert_eq!(
        code(device.save_profile(ID, &request(), true, expired, Some(&root), 0)),
        "deviceError:timeout"
    );
    // Less than 15 s of the caller's budget left: not started.
    let short = Some(Instant::now() + Duration::from_secs(14));
    assert_eq!(
        code(device.save_profile(ID, &request(), true, short, Some(&root), 0)),
        "deviceError:timeout"
    );
    for bad in [
        SaveRequest {
            dpi_levels: Some(vec![400, 800, 1200, 1600]),
            ..SaveRequest::default()
        },
        SaveRequest {
            report_rate_hz: Some(333),
            ..SaveRequest::default()
        },
        SaveRequest {
            report_rate_hz: Some(0),
            ..SaveRequest::default()
        },
    ] {
        assert_eq!(
            code(device.save_profile(ID, &bad, true, far(), Some(&root), 0)),
            "outOfRange"
        );
    }
    assert_eq!(handle.borrow().sent.len(), sent);
    assert_eq!(sim.borrow().calls.len(), before);
    assert!(!root.join(ID).exists(), "a backup was taken");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_sleeping_mouse_is_disconnected() {
    let root = temp_dir();
    let (mut device, handle, sim) = save_device(3, 1, true);
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    step(&mut device);
    let before = sim.borrow().calls.len();
    assert_eq!(code(save(&mut device, &root, true)), "disconnected");
    assert_eq!(sim.borrow().calls.len(), before);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn profile_2_active_is_not_writable() {
    let root = temp_dir();
    let (mut device, _handle, sim) = save_device(2, 1, true);
    assert_eq!(code(save(&mut device, &root, true)), "profileNotWritable");
    assert!(calls(&sim, 6).is_empty());
    assert!(!root.join(ID).exists());

    // Profile 3 active but disabled in the directory.
    let (mut device, _handle, sim) = save_device(3, 1, false);
    assert_eq!(code(save(&mut device, &root, true)), "profileNotWritable");
    assert!(calls(&sim, 6).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_mouse_in_host_mode_that_is_not_ours_is_not_onboard() {
    let root = temp_dir();
    let (mut device, _handle, sim) = save_device(3, 2, true);
    assert!(!device.host_by_us());
    assert_eq!(code(save(&mut device, &root, true)), "notOnboardMode");
    assert!(calls(&sim, 6).is_empty());
    assert!(calls(&sim, 1).is_empty(), "the mode was changed");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn our_host_mode_is_returned_first_and_then_the_profile_is_saved() {
    let root = temp_dir();
    let (mut device, _handle, sim) = save_device(3, 2, true);
    device.set_host_by_us(true);
    let outcome = save(&mut device, &root, true).expect("save");
    assert!(outcome.written);
    let mode_calls = calls(&sim, 1);
    assert_eq!(mode_calls.len(), 1);
    assert_eq!(mode_calls[0][0], 1, "returned to onboard");
    assert_eq!(sim.borrow().mode, 1);
    assert!(position(&sim, 1) < position(&sim, 6), "return before write");
    assert!(!device.host_by_us());
}

#[test]
fn a_backup_that_fails_stops_before_any_write_frame() {
    let root = temp_dir();
    let (mut device, _handle, sim) = save_device(3, 1, true);
    let original = sim.borrow().flash[&3].clone();
    // The backup root is a file: its directory cannot be created.
    let file = root.join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(code(save(&mut device, &file, true)), "backupFailed");
    for function in [3u8, 6, 7, 8] {
        assert!(calls(&sim, function).is_empty(), "fn{function} was sent");
    }
    assert_eq!(sim.borrow().flash[&3], original);
    // No app data directory at all.
    let result = device.save_profile(ID, &request(), true, far(), None, 0);
    assert_eq!(code(result), "backupFailed");
    assert!(calls(&sim, 6).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------- error mapping

fn detail_with(restore: RestoreStatus) -> Box<FailureDetail> {
    Box::new(FailureDetail {
        log: SequenceLog::default(),
        readback: ReadbackCheck::Identical,
        restore,
    })
}

fn mapped(e: SaveError) -> String {
    match classify_save_error(e) {
        WriteFailure::Refused(code) => code,
        WriteFailure::Lost(_) => "lost".into(),
    }
}

#[test]
fn save_errors_map_to_the_contract_codes() {
    assert_eq!(mapped(SaveError::NotOnboardMode), "notOnboardMode");
    assert_eq!(
        mapped(SaveError::ProfileNotWritable {
            active: 2,
            reason: "x"
        }),
        "profileNotWritable"
    );
    assert_eq!(
        mapped(SaveError::Write(SectorWriteError::Backup("disk".into()))),
        "backupFailed"
    );
    for refusal in [
        PatchError::DpiNotAllowed(5),
        PatchError::DpiIndexOutOfRange {
            field: "default",
            value: 9,
        },
        PatchError::DpiLevelZero {
            field: "shift",
            index: 1,
        },
        PatchError::RateNotAllowed(3),
    ] {
        assert_eq!(
            mapped(SaveError::Write(SectorWriteError::Patch(refusal))),
            "outOfRange"
        );
    }
    for (status, tag) in [
        (RestoreStatus::NotAttempted, "unknown"),
        (RestoreStatus::NotNeeded, "notNeeded"),
        (RestoreStatus::Restored(SequenceLog::default()), "restored"),
        (
            RestoreStatus::Failed("x".into(), SequenceLog::default()),
            "restoreFailed",
        ),
    ] {
        let failed = SectorWriteError::WriteFailed {
            cause: SessionError::Timeout,
            detail: detail_with(status.clone()),
        };
        assert_eq!(
            mapped(SaveError::Write(failed)),
            format!("flashWriteFailed:{tag}")
        );
        let verify = SectorWriteError::VerifyFailed {
            first_diff: Some(3),
            crc_ok: true,
            detail: detail_with(status),
        };
        assert_eq!(
            mapped(SaveError::Write(verify)),
            format!("flashWriteFailed:{tag}")
        );
    }
    // The data is on the flash but fn3 failed.
    let reselect = SaveError::Reselect {
        report: Box::new(logitech_g502x::WriteReport {
            sector: 3,
            read_bytes: Vec::new(),
            image: Vec::new(),
            log: SequenceLog::default(),
            outcome: logitech_g502x::WriteOutcome::Written,
        }),
        cause: SectorWriteError::SelectVerifyFailed {
            expected: 3,
            got: 2,
        },
        active_now: Some(2),
    };
    assert_eq!(mapped(reselect), "savedReselectFailed:0x0002");
    let unknown_now = SaveError::Reselect {
        report: Box::new(logitech_g502x::WriteReport {
            sector: 3,
            read_bytes: Vec::new(),
            image: Vec::new(),
            log: SequenceLog::default(),
            outcome: logitech_g502x::WriteOutcome::Written,
        }),
        cause: SectorWriteError::ReadCrcInvalid,
        active_now: None,
    };
    assert_eq!(mapped(unknown_now), "savedReselectFailed:unknown");
    assert_eq!(
        mapped(SaveError::Write(SectorWriteError::SelectVerifyFailed {
            expected: 3,
            got: 2
        })),
        "savedReselectFailed:unknown"
    );
    // A link lost mid-sequence: nothing could be restored, state unknown, and
    // the code is not "disconnected".
    let lost = SectorWriteError::WriteFailed {
        cause: SessionError::Link(myhub_transport::LinkError::Hid("unplugged".into())),
        detail: detail_with(RestoreStatus::NotAttempted),
    };
    assert_eq!(mapped(SaveError::Write(lost)), "flashWriteFailed:unknown");
    // Everything else.
    assert_eq!(
        mapped(SaveError::Session(SessionError::Device {
            code: 2,
            family: ErrorFamily::Hidpp20,
            name: None
        })),
        "deviceError:2"
    );
    assert_eq!(
        mapped(SaveError::Session(SessionError::Timeout)),
        "disconnected"
    );
    assert_eq!(
        mapped(SaveError::Write(SectorWriteError::ReadCrcInvalid)),
        "profileNotWritable"
    );
    assert_eq!(
        mapped(SaveError::Write(SectorWriteError::ImageInvalid("x".into()))),
        "deviceError:encode"
    );
}

// ----------------------------------------------------------------- battery

fn status_event(bytes: [u8; 4]) -> Vec<u8> {
    let mut payload = vec![0x11, 0x01, IDX_BATTERY, 0x00];
    payload.extend_from_slice(&bytes);
    long(&payload)
}

#[test]
fn a_battery_event_updates_charging_and_external_power() {
    let (mut device, handle) = read_device();
    let before = device.detail().unwrap();
    let battery = before.battery.unwrap();
    assert_eq!(
        (battery.percent, battery.charging, battery.external_power),
        (55, ChargingState::Discharging, false)
    );

    // Plug in: `35 04 01 01` (observed).
    handle
        .borrow_mut()
        .inbox
        .push_back(status_event([0x35, 0x04, 0x01, 0x01]));
    step(&mut device);
    assert!(device.take_dirty());
    let after = device.detail().unwrap();
    let battery = after.battery.unwrap();
    assert_eq!(battery.percent, 53);
    assert_eq!(battery.level, Some(BatteryLevel::Good));
    assert_eq!(battery.charging, ChargingState::Charging);
    assert!(battery.external_power);
    assert_eq!(after.summary.battery_percent, Some(53));
    // The change is published as a detail event.
    assert!(changes(Some(&before), Some(&after)).detail);

    // Unplug.
    handle
        .borrow_mut()
        .inbox
        .push_back(status_event([0x35, 0x04, 0x00, 0x00]));
    step(&mut device);
    let battery = device.detail().unwrap().battery.unwrap();
    assert_eq!(battery.charging, ChargingState::Discharging);
    assert!(!battery.external_power);

    // A malformed event (percent above 100) is ignored.
    handle
        .borrow_mut()
        .inbox
        .push_back(status_event([0xff, 0x04, 0x01, 0x01]));
    step(&mut device);
    assert_eq!(device.detail().unwrap().battery.unwrap().percent, 53);
}

#[test]
fn the_periodic_battery_reply_carries_the_whole_status() {
    let (mut device, handle) = read_device();
    handle
        .borrow_mut()
        .inbox
        .push_back(status_event([0x35, 0x04, 0x04, 0x01]));
    step(&mut device);
    assert_eq!(
        device.detail().unwrap().battery.unwrap().charging,
        ChargingState::SlowCharging
    );
    // Due: the reply from the log is `37 04 00 00`.
    device.step(Instant::now() + BATTERY_REFRESH).expect("step");
    let battery = device.detail().unwrap().battery.unwrap();
    assert_eq!(battery.percent, 55);
    assert_eq!(battery.charging, ChargingState::Discharging);
    assert!(!battery.external_power);
}

// ------------------------------------------------------ host-mode fixes 1a-1e

fn switched_to_host() -> (Device<MockLink>, Handle) {
    let (mut device, handle) = read_device();
    onboard_mouse(&handle);
    accept_dpi(&handle, 1200, 1200);
    device.enter_temporary(ID, true, None).expect("enter");
    assert!(device.host_by_us());
    (device, handle)
}

#[test]
fn a_failed_return_keeps_the_mark_when_the_outcome_is_unknown() {
    // Lost reply.
    let (mut device, handle) = switched_to_host();
    handle.borrow_mut().silent.push(mode_set_tx(1));
    device.return_to_onboard().unwrap();
    assert!(device.host_by_us(), "timeout");

    // A rediscovery in the middle (0x41 queued before the frame).
    let (mut device, handle) = switched_to_host();
    handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
    device.return_to_onboard().unwrap();
    assert!(device.host_by_us(), "needs rediscovery");

    // HID++ 1.0 0x08: the mouse is off.
    let (mut device, handle) = switched_to_host();
    handle.borrow_mut().device_off = true;
    device.return_to_onboard().unwrap();
    assert!(device.host_by_us(), "0x08");

    // A device error answering fn1: a definite refusal, the attempt is spent.
    let (mut device, handle) = switched_to_host();
    handle.borrow_mut().replies.insert(
        mode_set_tx(1),
        long(&[0x11, 0x01, 0xff, IDX_ONBOARD, 0x1b, 0x02]),
    );
    device.return_to_onboard().unwrap();
    assert!(!device.host_by_us(), "device error");

    // A lost link.
    let (mut device, handle) = switched_to_host();
    handle.borrow_mut().broken = true;
    assert!(device.return_to_onboard().is_err());
    assert!(device.host_by_us(), "link");
}

#[test]
fn the_mark_stays_when_fn1_was_acked_but_the_read_back_failed() {
    let (mut device, handle) = read_device();
    onboard_mouse(&handle);
    // fn1 is ACKed, then fn2 answers with an error.
    handle.borrow_mut().then_reply.insert(
        mode_set_tx(2),
        vec![(
            hex(FN2_TX),
            long(&[0x11, 0x01, 0xff, IDX_ONBOARD, 0x2b, 0x02]),
        )],
    );
    assert_eq!(
        refused(device.enter_temporary(ID, true, None)),
        "notHostMode"
    );
    assert!(device.host_by_us());
}

#[test]
fn the_mark_stays_when_a_rediscovery_hits_the_switch() {
    let (mut device, handle) = read_device();
    onboard_mouse(&handle);
    // After the mode read a 0x41 is waiting: the fn1 exchange finds it.
    handle
        .borrow_mut()
        .push_after
        .insert(hex(FN2_TX), hex("10014111a29f40"));
    assert_eq!(
        refused(device.enter_temporary(ID, true, None)),
        "disconnected"
    );
    assert!(device.host_by_us());
}

#[test]
fn the_shutdown_budget_covers_the_return() {
    assert!(
        SHUTDOWN_WAIT >= WRITE_INTERVAL + SHUTDOWN_REPLY_TIMEOUT * 3 + Duration::from_millis(1500)
    );
}

#[test]
fn return_blind_discovers_again_and_returns_only_our_switch() {
    // Mark set: a fresh device over the same mouse returns it to onboard mode.
    let (mut device, handle) = mock_device();
    onboard_mouse(&handle);
    device.set_host_by_us(true);
    let before = handle.borrow().sent.len();
    return_blind(&mut device).unwrap();
    let sent = hex_frames(&handle, before);
    assert!(sent.iter().any(|f| f.starts_with("1101091b01")), "{sent:?}");
    assert!(!device.host_by_us());

    // Mark not set: nothing at all is sent.
    let (mut device, handle) = mock_device();
    onboard_mouse(&handle);
    return_blind(&mut device).unwrap();
    assert!(handle.borrow().sent.is_empty());
}

#[test]
fn supervise_says_whether_it_gave_up() {
    let mut ctx = 0u32;
    assert!(supervise(&mut ctx, |_| panic!("always"), |_| {}, |_| true));
    assert!(!supervise(&mut ctx, |_| {}, |_| {}, |_| true));
    // Stopping during the pause is not giving up.
    assert!(!supervise(&mut ctx, |_| panic!("once"), |_| {}, |_| false));
}

// ------------------------------------------------- fix round (flash, deadline)

#[test]
fn a_save_with_16_seconds_left_starts() {
    let root = temp_dir();
    let (mut device, _handle, sim) = save_device(3, 1, true);
    let enough = Some(Instant::now() + Duration::from_secs(16));
    let outcome = device
        .save_profile(ID, &request(), true, enough, Some(&root), 1)
        .expect("save");
    assert!(outcome.written);
    assert_eq!(calls(&sim, 6).len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_flash_flag_is_up_exactly_while_the_sector_is_written() {
    let root = temp_dir();
    let (mut device, handle, sim) = save_device(3, 1, true);
    let flag = Arc::clone(&device.flash_in_flight);
    let seen: Rc<RefCell<Vec<(u8, bool)>>> = Rc::default();
    let mut inner = hook(Rc::clone(&sim));
    {
        let (flag, seen) = (Arc::clone(&flag), Rc::clone(&seen));
        handle.borrow_mut().onboard_hook = Some(Box::new(move |report| {
            let function = report[3] >> 4;
            if (6..=8).contains(&function) {
                seen.borrow_mut()
                    .push((function, flag.load(Ordering::SeqCst)));
            }
            inner(report)
        }));
    }
    assert!(!flag.load(Ordering::SeqCst));
    save(&mut device, &root, true).expect("save");
    let seen = seen.borrow();
    assert_eq!(seen.len(), 18, "fn6, 16 x fn7, fn8");
    assert!(seen.iter().all(|(_, up)| *up), "the flag dropped mid-write");
    assert!(!flag.load(Ordering::SeqCst), "the flag stayed up");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_save_that_meets_the_shutdown_flag_does_not_start_and_leaves_the_flag_down() {
    let root = temp_dir();
    let (mut device, _handle, sim) = save_device(3, 1, true);
    let shutdown = Arc::new(AtomicBool::new(true));
    device = device.with_shutdown(shutdown);
    assert_eq!(code(save(&mut device, &root, true)), "disconnected");
    assert!(calls(&sim, 6).is_empty());
    assert!(!device.flash_in_flight.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shutdown_waits_for_a_flash_write_in_flight() {
    let flag = Arc::new(AtomicBool::new(true));
    let clearing = Arc::clone(&flag);
    let started = Instant::now();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        clearing.store(false, Ordering::SeqCst);
    });
    assert!(wait_for_flash(&flag, Duration::from_secs(5)));
    assert!(started.elapsed() >= Duration::from_millis(120));
    handle.join().unwrap();

    // A write that never ends is given up on after the limit.
    let stuck = AtomicBool::new(true);
    let started = Instant::now();
    assert!(!wait_for_flash(&stuck, Duration::from_millis(100)));
    assert!(started.elapsed() >= Duration::from_millis(100));
    // Nothing in flight: no wait at all.
    assert!(wait_for_flash(&AtomicBool::new(false), Duration::ZERO));
    assert_eq!(FLASH_SHUTDOWN_WAIT, Duration::from_secs(60));
}

#[test]
fn a_return_whose_read_back_fails_keeps_the_mark() {
    let (mut device, handle) = switched_to_host();
    // fn1 is ACKed but the mouse still reports host mode: VerifyFailed.
    {
        let mut mock = handle.borrow_mut();
        mock.then_reply.clear();
        mock.replies.insert(hex(FN2_TX), mode_reply(2));
    }
    device.return_to_onboard().unwrap();
    assert!(device.host_by_us());
}

// -------------------------------------------- fix round 2 (link lost on save)

#[test]
fn a_link_lost_during_the_reread_keeps_the_save_result() {
    let root = temp_dir();
    let (mut device, handle, sim) = save_device(3, 1, true);
    // After the last verify read (fn4: the save, the switch away, the switch back)
    // the link is gone: the re-read that follows fails with a link error.
    handle.borrow_mut().break_after = Some((4, 3));
    let (result, lost) =
        device.save_profile_attempt(ID, &request(), true, far(), Some(&root), 1_760_520_605);
    let outcome = result.expect("the write itself succeeded");
    assert!(outcome.written);
    assert!(outcome.reloaded);
    assert!(outcome.backup_path.is_some());
    assert_eq!(calls(&sim, 8).len(), 1);
    // The worker still learns that the link is gone.
    assert!(lost.is_some(), "the lost link was swallowed");
    // The detail is the last known one (the read failed, nothing was cleared).
    assert!(device.detail().is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_link_lost_mid_sequence_is_flash_unknown_and_still_reaches_the_worker() {
    let root = temp_dir();
    let (mut device, handle, sim) = save_device(3, 1, true);
    // The link breaks after the 5th data frame.
    handle.borrow_mut().break_after = Some((7, 5));
    let (result, lost) =
        device.save_profile_attempt(ID, &request(), true, far(), Some(&root), 1_760_520_605);
    assert_eq!(code(result), "flashWriteFailed:unknown");
    assert!(lost.is_some());
    assert!(
        calls(&sim, 7).len() < 16,
        "the sequence ran on after the break"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn link_errors_in_the_readback_or_the_restore_are_unknown_too() {
    // Built from the real error text, so a change to it breaks this test.
    let link =
        || SessionError::Link(myhub_transport::LinkError::Hid("unplugged".into())).to_string();
    // Restore failed because the link broke.
    let failed = SectorWriteError::WriteFailed {
        cause: SessionError::Timeout,
        detail: detail_with(RestoreStatus::Failed(link(), SequenceLog::default())),
    };
    assert_eq!(mapped(SaveError::Write(failed)), "flashWriteFailed:unknown");
    // The read-back after the failure was unreadable because of the link.
    let unreadable = SectorWriteError::VerifyFailed {
        first_diff: None,
        crc_ok: false,
        detail: Box::new(FailureDetail {
            log: SequenceLog::default(),
            readback: ReadbackCheck::Unreadable(link()),
            restore: RestoreStatus::NotAttempted,
        }),
    };
    assert_eq!(
        mapped(SaveError::Write(unreadable)),
        "flashWriteFailed:unknown"
    );
    // A restore that failed for another reason stays restoreFailed.
    let other = SectorWriteError::VerifyFailed {
        first_diff: Some(1),
        crc_ok: true,
        detail: detail_with(RestoreStatus::Failed(
            "read-back differs".into(),
            SequenceLog::default(),
        )),
    };
    assert_eq!(
        mapped(SaveError::Write(other)),
        "flashWriteFailed:restoreFailed"
    );
}

// ------------------------------------------------- 0x8100 notifications

fn onboard_event(function: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x11, 0x01, IDX_ONBOARD, function << 4];
    frame.extend_from_slice(payload);
    long(&frame)
}

fn fn4_tx() -> Vec<u8> {
    long(&[0x11, 0x01, IDX_ONBOARD, 0x4b])
}

/// A mouse in onboard mode with profile 3 enabled and levels 400..2800
/// (default index 1), profile 2 active, as read by the last snapshot. The mock
/// answers the confirm read like the mouse would after the events below.
fn event_device() -> (Device<MockLink>, Handle) {
    let (mut device, handle) = read_device();
    {
        let snap = device.snapshot.as_mut().unwrap();
        snap.onboard.mode_raw_hex = Some("01000000".into());
        for entry in &mut snap.onboard.directory.entries {
            entry.enabled |= entry.sector == 3;
        }
        let p3 = snap
            .onboard
            .profiles
            .iter_mut()
            .find(|p| p.sector == "0x0003")
            .and_then(|p| p.profile.as_mut())
            .unwrap();
        p3.dpi_levels = [400, 800, 1200, 2000, 2800];
        p3.default_dpi_index = 1;
    }
    {
        let mut mock = handle.borrow_mut();
        mock.replies
            .insert(fn4_tx(), long(&[0x11, 0x01, IDX_ONBOARD, 0x4b, 0x00, 0x03]));
        mock.replies.insert(dpi_get_tx(), dpi_get_reply(800));
    }
    device.take_dirty();
    (device, handle)
}

/// Routes the queued frames and applies the notifications, without a step.
fn deliver(device: &mut Device<MockLink>) {
    device
        .session
        .poll_events(Duration::from_millis(5))
        .unwrap();
    device.apply_notifications(Instant::now());
}

fn onboard_of(device: &Device<MockLink>) -> myhub_core::OnboardView {
    device.detail().unwrap().onboard.unwrap()
}

#[test]
fn a_profile_event_updates_profile_writable_profile_and_dpi_then_confirms() {
    let (mut device, handle) = event_device();
    let before = device.detail().unwrap();
    assert_eq!(onboard_of(&device).active_profile, Some(2));
    assert_eq!(onboard_of(&device).writable_profile, None);
    let sent = handle.borrow().sent.len();

    handle
        .borrow_mut()
        .inbox
        .push_back(onboard_event(0, &[0x00, 0x03]));
    deliver(&mut device);
    let view = onboard_of(&device);
    assert_eq!(view.active_profile, Some(3));
    assert_eq!(view.writable_profile, Some(3));
    // The new profile default level (index 1) is the current DPI.
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 800);
    assert!(device.take_dirty());
    assert!(device.confirm_pending);
    assert_eq!(handle.borrow().sent.len(), sent, "applying sends nothing");
    assert!(changes(Some(&before), device.detail().as_ref()).detail);

    // The confirm read is the two read-tier requests, not a snapshot.
    step(&mut device);
    let frames = hex_frames(&handle, sent);
    assert_eq!(
        frames,
        [
            "1101072b00000000000000000000000000000000",
            "1101094b00000000000000000000000000000000"
        ]
    );
    assert!(!device.confirm_pending);
    assert_eq!(onboard_of(&device).active_profile, Some(3));
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 800);
    // Asked once only.
    step(&mut device);
    assert_eq!(handle.borrow().sent.len(), sent + 2);
}

#[test]
fn a_dpi_index_event_sets_the_level_of_the_active_profile() {
    let (mut device, handle) = event_device();
    // Make profile 3 the active one first.
    handle
        .borrow_mut()
        .inbox
        .push_back(onboard_event(0, &[0x00, 0x03]));
    step(&mut device);
    device.take_dirty();
    let sent = handle.borrow().sent.len();

    for (index, dpi) in [(3u8, 2000u16), (0, 400), (4, 2800), (2, 1200)] {
        handle
            .borrow_mut()
            .inbox
            .push_back(onboard_event(1, &[index]));
        deliver(&mut device);
        assert_eq!(
            device.detail().unwrap().dpi.unwrap().current,
            dpi,
            "index {index}"
        );
        assert!(device.take_dirty());
        assert!(!device.confirm_pending, "a known index needs no read");
    }
    assert_eq!(handle.borrow().sent.len(), sent);
}

#[test]
fn an_index_out_of_range_schedules_the_confirm_read() {
    for index in [5u8, 9, 255] {
        let (mut device, handle) = event_device();
        handle
            .borrow_mut()
            .replies
            .insert(dpi_get_tx(), dpi_get_reply(1234));
        let sent = handle.borrow().sent.len();
        handle
            .borrow_mut()
            .inbox
            .push_back(onboard_event(1, &[index]));
        deliver(&mut device);
        assert!(device.confirm_pending, "index {index}");
        assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1600);
        assert_eq!(handle.borrow().sent.len(), sent);
        step(&mut device);
        assert_eq!(handle.borrow().sent.len(), sent + 2, "the two reads");
        assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1234);
    }
}

#[test]
fn unknown_events_are_ignored_and_logged_once() {
    let (mut device, handle) = event_device();
    let sent = handle.borrow().sent.len();
    for function in [2u8, 3, 7] {
        handle
            .borrow_mut()
            .inbox
            .push_back(onboard_event(function, &[1, 2, 3]));
        deliver(&mut device);
        assert!(!device.take_dirty());
        assert!(!device.confirm_pending);
    }
    assert!(device.unknown_event_logged);
    assert_eq!(handle.borrow().sent.len(), sent);
    assert_eq!(onboard_of(&device).active_profile, Some(2));
}

#[test]
fn events_from_another_device_or_feature_are_ignored() {
    let (mut device, handle) = event_device();
    let sent = handle.borrow().sent.len();
    // Another device behind the receiver, same feature index.
    handle
        .borrow_mut()
        .inbox
        .push_back(long(&[0x11, 0x02, IDX_ONBOARD, 0x00, 0x00, 0x03]));
    // Our device, another feature index (0x2201 here).
    handle
        .borrow_mut()
        .inbox
        .push_back(long(&[0x11, 0x01, IDX_DPI, 0x10, 0x03]));
    handle
        .borrow_mut()
        .inbox
        .push_back(long(&[0x11, 0x01, IDX_DPI, 0x00, 0x00, 0x03]));
    deliver(&mut device);
    assert!(!device.take_dirty());
    assert!(!device.confirm_pending);
    assert_eq!(onboard_of(&device).active_profile, Some(2));
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1600);
    assert_eq!(handle.borrow().sent.len(), sent);
}

#[test]
fn events_are_skipped_while_the_feature_table_is_cleared() {
    let (mut device, handle) = event_device();
    // Switched off: the 0x41 clears the table before the event is looked at.
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    handle
        .borrow_mut()
        .inbox
        .push_back(onboard_event(0, &[0x00, 0x03]));
    handle.borrow_mut().inbox.push_back(onboard_event(1, &[3]));
    deliver(&mut device);
    assert!(device.session.table().is_empty());
    assert!(!device.confirm_pending);
    assert_eq!(onboard_of(&device).active_profile, Some(2));
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1600);
}

// ------------------------------------------------ review should-fixes

#[test]
fn without_another_enabled_profile_the_save_reports_reloaded_false() {
    let root = temp_dir();
    let (mut device, _handle, sim) = save_device_with(3, 1, &[3]);
    let outcome = save(&mut device, &root, true).expect("save");
    assert!(outcome.written);
    assert!(!outcome.reloaded, "nothing to switch to");
    assert_eq!(calls(&sim, 8).len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_profile_event_for_a_sector_that_is_no_profile_only_schedules_a_read() {
    for sector in [0u16, 6, 0x0100, 0x0106, 0x00ff, 0xffff] {
        let (mut device, handle) = event_device();
        let sent = handle.borrow().sent.len();
        let [hi, lo] = sector.to_be_bytes();
        handle
            .borrow_mut()
            .inbox
            .push_back(onboard_event(0, &[hi, lo]));
        deliver(&mut device);
        assert!(device.confirm_pending, "{sector:#06x}");
        assert!(!device.take_dirty(), "{sector:#06x} was applied");
        assert_eq!(onboard_of(&device).active_profile, Some(2));
        assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1600);
        assert_eq!(handle.borrow().sent.len(), sent);
    }
    // The ROM sectors and the user sectors 1..=5 are applied.
    for (sector, applied) in [(1u16, true), (5, true), (0x0101, true), (0x0105, true)] {
        let (mut device, handle) = event_device();
        let [hi, lo] = sector.to_be_bytes();
        handle
            .borrow_mut()
            .inbox
            .push_back(onboard_event(0, &[hi, lo]));
        deliver(&mut device);
        assert_eq!(device.active_sector(), Some(sector), "{sector:#06x}");
        assert!(device.confirm_pending);
        assert_eq!(device.take_dirty(), applied, "{sector:#06x}");
    }
}

// ------------------------------------------- review fixes (temporary mode)

#[test]
fn a_write_needs_the_snapshot_to_say_host_besides_the_mark() {
    let (mut device, handle) = read_device();
    // The mark is set but the snapshot says onboard (fn2 = 01): no frame.
    device.set_host_by_us(true);
    device.apply_mode(1);
    let from = handle.borrow().sent.len();
    assert_eq!(
        refused(device.write(ID, WriteOp::Dpi(1200), true)),
        "notHostMode"
    );
    assert_eq!(
        refused(device.write(ID, WriteOp::ReportRate(500), true)),
        "notHostMode"
    );
    assert!(hex_frames(&handle, from).is_empty());

    // Mark and a host snapshot: the write goes out.
    device.apply_mode(2);
    accept_dpi(&handle, 1200, 1200);
    device.write(ID, WriteOp::Dpi(1200), true).expect("write");
}

#[test]
fn leaving_a_mouse_that_is_not_ours_needs_writes_allowed() {
    // Host mode, no mark, writes off: refused, not even the mode is read.
    let (mut device, handle) = read_device();
    let from = handle.borrow().sent.len();
    assert_eq!(
        refused(device.leave_temporary(ID, false, None)),
        "writesDisabled"
    );
    assert!(hex_frames(&handle, from).is_empty());
    assert!(!device.host_by_us());

    // With our mark the return needs no setting (it undoes our own switch).
    let (mut device, handle) = read_device();
    {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(mode_set_tx(1), mode_set_tx(1));
        mock.then_reply
            .insert(mode_set_tx(1), vec![(hex(FN2_TX), mode_reply(1))]);
    }
    device.set_host_by_us(true);
    device.leave_temporary(ID, false, None).expect("leave");
    assert!(!device.host_by_us());
    assert_eq!(mode_of(&device), Some(OnboardMode::Onboard));
}

#[test]
fn leaving_when_fn2_is_neither_onboard_nor_host_is_verify_failed() {
    let (mut device, handle) = read_device();
    handle
        .borrow_mut()
        .replies
        .insert(hex(FN2_TX), mode_reply(3));
    let from = handle.borrow().sent.len();
    assert_eq!(
        refused(device.leave_temporary(ID, true, None)),
        "verifyFailed"
    );
    // The spy stop and the mode read went out; nothing was switched.
    assert_eq!(hex_frames(&handle, from).len(), 2);
    assert!(!device.host_by_us());
}

// ----------------------------------------- DPI buttons via the button spy

const IDX_SPY: u8 = 10;

fn spy_tx(start: bool) -> Vec<u8> {
    long(&[0x11, 0x01, IDX_SPY, if start { 0x1b } else { 0x2b }])
}

fn spy_event(mask: u16) -> Vec<u8> {
    let [lo, hi] = mask.to_le_bytes();
    long(&[0x11, 0x01, IDX_SPY, 0x00, lo, hi])
}

/// An onboard mouse (profile 2 active, levels 400..3200, 1600 DPI) that has
/// entered temporary mode: host mark set and the spy running.
fn spy_device() -> (Device<MockLink>, Handle) {
    let (mut device, handle) = read_device();
    {
        let snap = device.snapshot.as_mut().unwrap();
        let p2 = snap
            .onboard
            .profiles
            .iter_mut()
            .find(|p| p.sector == "0x0002")
            .and_then(|p| p.profile.as_mut())
            .unwrap();
        p2.dpi_levels = [400, 800, 1200, 1600, 3200];
    }
    onboard_mouse(&handle);
    device.enter_temporary(ID, true, None).expect("enter");
    assert!(device.spy_active());
    device.take_dirty();
    (device, handle)
}

fn press(device: &mut Device<MockLink>, handle: &Handle, masks: &[u16]) {
    for mask in masks {
        handle.borrow_mut().inbox.push_back(spy_event(*mask));
    }
    deliver(device);
}

fn set_frames(handle: &Handle, from: usize) -> Vec<String> {
    hex_frames(handle, from)
        .into_iter()
        .filter(|f| f.starts_with("1101073b"))
        .collect()
}

#[test]
fn entering_remembers_the_active_profile_and_starts_the_spy() {
    let (device, _handle) = spy_device();
    assert_eq!(device.remembered_profile, Some(2));
    assert!(device.host_by_us());
}

#[test]
fn a_dpi_up_edge_applies_the_next_level_and_only_edges_count() {
    let (mut device, handle) = spy_device();
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1600);
    let before = device.detail().unwrap();

    // DPI up (bit 1): 1600 -> 3200.
    press(&mut device, &handle, &[0x0002]);
    assert_eq!(device.dpi_target, Some(3200));
    accept_dpi(&handle, 3200, 3200);
    let from = handle.borrow().sent.len();
    device.apply_dpi_target(true).unwrap();
    assert_eq!(
        set_frames(&handle, from),
        ["1101073b000c8000000000000000000000000000"]
    );
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 3200);
    assert!(device.take_dirty());
    assert!(changes(Some(&before), device.detail().as_ref()).detail);

    // The same mask again (still held) and other bits do nothing.
    press(&mut device, &handle, &[0x0002, 0x0002]);
    assert_eq!(device.dpi_target, None);
    press(
        &mut device,
        &handle,
        &[0x0000, 0x0001, 0x0000, 0x0100, 0x2000, 0x0000],
    );
    assert_eq!(device.dpi_target, None);

    // At the top level a new press is clamped: nothing to apply.
    press(&mut device, &handle, &[0x0000, 0x0002]);
    assert_eq!(device.dpi_target, None);

    // DPI down (bit 2): 3200 -> 1600, then down to the bottom and clamped.
    press(&mut device, &handle, &[0x0000, 0x0004]);
    assert_eq!(device.dpi_target, Some(1600));
    accept_dpi(&handle, 1600, 1600);
    device.apply_dpi_target(true).unwrap();
    for (dpi, _) in [(1200u16, 0), (800, 0), (400, 0)] {
        press(&mut device, &handle, &[0x0000, 0x0004]);
        assert_eq!(device.dpi_target, Some(dpi));
        accept_dpi(&handle, dpi, dpi);
        device.apply_dpi_target(true).unwrap();
    }
    press(&mut device, &handle, &[0x0000, 0x0004]);
    assert_eq!(device.dpi_target, None, "clamped at the bottom");
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 400);
}

#[test]
fn a_current_dpi_between_levels_goes_to_the_neighbouring_level() {
    let (mut device, handle) = spy_device();
    device.snapshot.as_mut().unwrap().dpi.current = 1550;
    press(&mut device, &handle, &[0x0002]);
    assert_eq!(device.dpi_target, Some(1600));
    device.dpi_target = None;
    press(&mut device, &handle, &[0x0000, 0x0004]);
    assert_eq!(device.dpi_target, Some(1200));
}

#[test]
fn of_two_fast_presses_the_latest_target_wins_with_one_write() {
    let (mut device, handle) = spy_device();
    device.snapshot.as_mut().unwrap().dpi.current = 400;
    // Two presses before the worker gets to write: 400 -> 800 -> 1200.
    press(&mut device, &handle, &[0x0002, 0x0000, 0x0002, 0x0000]);
    assert_eq!(device.dpi_target, Some(1200));
    accept_dpi(&handle, 1200, 1200);
    let from = handle.borrow().sent.len();
    device.apply_dpi_target(true).unwrap();
    assert_eq!(
        set_frames(&handle, from),
        ["1101073b0004b000000000000000000000000000"],
        "one write, no step through 800"
    );
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1200);
    assert_eq!(device.dpi_target, None);
}

#[test]
fn a_press_with_writes_off_or_unknown_levels_changes_nothing() {
    // Writes off: the target is dropped, nothing is sent.
    let (mut device, handle) = spy_device();
    press(&mut device, &handle, &[0x0002]);
    let from = handle.borrow().sent.len();
    device.apply_dpi_target(false).unwrap();
    assert!(hex_frames(&handle, from).is_empty());
    assert_eq!(device.dpi_target, None);

    // No remembered profile and no active user profile: ignored, logged once.
    let (mut device, handle) = spy_device();
    device.remembered_profile = None;
    device
        .snapshot
        .as_mut()
        .unwrap()
        .onboard
        .active_profile_raw_hex = Some("00000000".into());
    press(&mut device, &handle, &[0x0002, 0x0000, 0x0004]);
    assert_eq!(device.dpi_target, None);
    assert!(device.spy_unknown_logged);

    // Events while the spy is not running are ignored.
    let (mut device, handle) = spy_device();
    device.set_spy_active(false);
    press(&mut device, &handle, &[0x0002]);
    assert_eq!(device.dpi_target, None);
}

#[test]
fn a_failed_spy_start_keeps_host_mode_and_does_not_fail_the_enter() {
    let (mut device, handle) = read_device();
    onboard_mouse(&handle);
    handle
        .borrow_mut()
        .replies
        .insert(spy_tx(true), long(&[0x11, 0x01, 0xff, IDX_SPY, 0x1b, 0x02]));
    device.enter_temporary(ID, true, None).expect("enter");
    assert!(device.host_by_us(), "host mode stays");
    assert!(!device.spy_active(), "the DPI buttons do not work");
    assert_eq!(mode_of(&device), Some(OnboardMode::Host));
}

#[test]
fn a_link_up_with_the_mark_starts_the_spy_again() {
    let (mut device, handle) = spy_device();
    let starts = |handle: &Handle| {
        handle
            .borrow()
            .sent
            .iter()
            .filter(|f| f.as_slice() == spy_tx(true).as_slice())
            .count()
    };
    assert_eq!(starts(&handle), 1);
    device.take_restore_due();
    // The mouse stays in host mode over a power cycle [OBS].
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    step(&mut device);
    handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
    step(&mut device);
    step(&mut device);
    assert!(device.host_by_us());
    assert_eq!(starts(&handle), 2, "the spy was not restarted");
    assert!(device.spy_active());
}

#[test]
fn no_spy_restart_when_the_mark_is_not_set() {
    let (mut device, handle) = read_device();
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    step(&mut device);
    handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
    step(&mut device);
    step(&mut device);
    assert!(
        !handle
            .borrow()
            .sent
            .iter()
            .any(|f| f.as_slice() == spy_tx(true).as_slice())
    );
}

#[test]
fn shutdown_stops_the_spy_before_the_return() {
    let (mut device, handle) = spy_device();
    device = device.with_shutdown(Arc::new(AtomicBool::new(true)));
    let from = handle.borrow().sent.len();
    device.return_to_onboard().unwrap();
    assert_eq!(
        hex_frames(&handle, from),
        [
            "11010a2b00000000000000000000000000000000",
            "1101091b01000000000000000000000000000000",
            "1101092b00000000000000000000000000000000",
        ]
    );
    assert!(!device.spy_active());
    assert!(!device.host_by_us());
}

#[test]
fn every_host_leaving_path_stops_the_spy_first() {
    // Writes turned off, and the return before a save, both go through
    // return_to_onboard; so does the last-resort return (fresh device, mark set).
    let (mut device, handle) = spy_device();
    let from = handle.borrow().sent.len();
    return_if_writes_off_for_test(&mut device);
    let sent = hex_frames(&handle, from);
    assert!(sent[0].starts_with("11010a2b"), "{sent:?}");

    let (mut device, handle) = mock_device();
    onboard_mouse(&handle);
    device.set_host_by_us(true);
    let from = handle.borrow().sent.len();
    return_blind(&mut device).unwrap();
    let sent = hex_frames(&handle, from);
    let stop = sent.iter().position(|f| f.starts_with("11010a2b")).unwrap();
    let switch = sent
        .iter()
        .position(|f| f.starts_with("1101091b01"))
        .unwrap();
    assert!(stop < switch, "{sent:?}");
}

/// Writes off after a full read: the same call the worker makes.
fn return_if_writes_off_for_test(device: &mut Device<MockLink>) {
    device.fresh_read = true;
    device.snapshot.as_mut().unwrap().onboard.mode_raw_hex = Some("02000000".into());
    return_if_writes_off(device, &settings_with(false, false)).unwrap();
}

// ------------------------------------------- spy stop retry, stale flag, marks

fn stops(handle: &Handle) -> usize {
    handle
        .borrow()
        .sent
        .iter()
        .filter(|f| f.as_slice() == spy_tx(false).as_slice())
        .count()
}

#[test]
fn a_failed_spy_stop_is_retried_once_and_a_good_retry_clears_the_flag() {
    let (mut device, handle) = spy_device();
    // First answer: a device error; the retry gets the normal reply.
    {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(
            spy_tx(false),
            long(&[0x11, 0x01, 0xff, IDX_SPY, 0x2b, 0x02]),
        );
        mock.then_reply
            .insert(spy_tx(false), vec![(spy_tx(false), spy_tx(false))]);
    }
    let from = stops(&handle);
    device.stop_spy().unwrap();
    assert_eq!(stops(&handle) - from, 2, "one retry");
    assert!(!device.spy_stale());
}

#[test]
fn a_stop_that_keeps_failing_leaves_the_flag_and_is_sent_again_later() {
    let (mut device, handle) = spy_device();
    handle.borrow_mut().silent.push(spy_tx(false));
    let before = stops(&handle);
    device.stop_spy().unwrap();
    assert_eq!(stops(&handle) - before, 2, "tried twice");
    assert!(device.spy_stale(), "the spy may still run");
    assert!(!device.spy_active());

    // Not retried at once from the pass loop ...
    let after = stops(&handle);
    step(&mut device);
    assert_eq!(stops(&handle), after);

    // ... but on a later reachable pass, and the flag clears on success.
    handle.borrow_mut().silent.clear();
    device.step(Instant::now() + SPY_STALE_RETRY).unwrap();
    assert_eq!(stops(&handle), after + 1);
    assert!(!device.spy_stale());
}

#[test]
fn a_stale_spy_is_stopped_again_on_leave() {
    let (mut device, handle) = spy_device();
    handle.borrow_mut().silent.push(spy_tx(false));
    // Leaving stops the spy first; the stop gets no answer (twice).
    let _ = device.leave_temporary(ID, true, None);
    assert!(device.spy_stale());
    // The next leave (the user clicks again) sends the stop again.
    handle.borrow_mut().silent.clear();
    let before = stops(&handle);
    device.leave_temporary(ID, true, None).expect("leave");
    assert!(stops(&handle) > before);
    assert!(!device.spy_stale());
}

#[test]
fn the_stale_flag_survives_a_device_drop() {
    let flag = Arc::new(AtomicBool::new(false));
    let (mut device, handle) = spy_device();
    device = device.with_spy_stale(Arc::clone(&flag));
    handle.borrow_mut().silent.push(spy_tx(false));
    device.stop_spy().unwrap();
    assert!(flag.load(Ordering::SeqCst));
    drop(device);
    assert!(flag.load(Ordering::SeqCst));
}

#[test]
fn leave_without_the_mark_stops_the_spy_before_the_mode_read_in_any_mode() {
    for onboard in [true, false] {
        let (mut device, handle) = read_device();
        if onboard {
            onboard_mouse(&handle);
        } else {
            let mut mock = handle.borrow_mut();
            mock.replies.insert(mode_set_tx(1), mode_set_tx(1));
            mock.then_reply
                .insert(mode_set_tx(1), vec![(hex(FN2_TX), mode_reply(1))]);
        }
        let from = handle.borrow().sent.len();
        device.leave_temporary(ID, true, None).expect("leave");
        let sent = hex_frames(&handle, from);
        assert!(sent[0].starts_with("11010a2b"), "{sent:?}");
        assert!(sent[1].starts_with("1101092b"), "{sent:?}");
    }
    // With writes off nothing is sent at all.
    let (mut device, handle) = read_device();
    let from = handle.borrow().sent.len();
    assert_eq!(
        refused(device.leave_temporary(ID, false, None)),
        "writesDisabled"
    );
    assert!(hex_frames(&handle, from).is_empty());
}

#[test]
fn a_mode_1_read_clears_the_spy_mark_and_the_remembered_profile() {
    let (mut device, handle) = spy_device();
    assert!(device.spy_active());
    assert_eq!(device.remembered_profile, Some(2));
    device.take_restore_due();
    // The mouse reset itself: onboard again after the power cycle.
    handle
        .borrow_mut()
        .replies
        .insert(hex(FN2_TX), mode_reply(1));
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    step(&mut device);
    handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
    step(&mut device);
    step(&mut device);
    assert!(!device.host_by_us());
    assert!(!device.spy_active());
    assert_eq!(device.remembered_profile, None);
    // No restart of the spy for a mouse that is onboard.
    assert_eq!(
        handle
            .borrow()
            .sent
            .iter()
            .filter(|f| f.as_slice() == spy_tx(true).as_slice())
            .count(),
        1
    );
}

#[test]
fn the_remembered_profile_goes_with_the_mark_but_survives_a_failed_return() {
    // A successful return clears it.
    let (mut device, _handle) = spy_device();
    device.return_to_onboard().unwrap();
    assert_eq!(device.remembered_profile, None);

    // A lost reply keeps the mark, and with it the remembered profile.
    let (mut device, handle) = spy_device();
    handle.borrow_mut().silent.push(mode_set_tx(1));
    device.return_to_onboard().unwrap();
    assert!(device.host_by_us());
    assert_eq!(device.remembered_profile, Some(2));
}

// ------------------------------------------------ restore last mode at start

fn host_prefs() -> DevicePrefs {
    DevicePrefs {
        dpi: Some(1200),
        report_rate_hz: Some(500),
        mode: OnboardMode::Host,
        profile: None,
    }
}

fn rate_set_tx(ms: u8) -> Vec<u8> {
    long(&[0x11, 0x01, 0x0b, 0x2b, ms])
}

fn rate_get_tx() -> Vec<u8> {
    long(&[0x11, 0x01, 0x0b, 0x1b])
}

/// An onboard mouse just read (the first read after start), the mock ready for
/// the whole restore: host switch, spy, DPI 1200 and 500 Hz.
fn start_device() -> (Device<MockLink>, Handle) {
    let (mut device, handle) = mock_device();
    onboard_mouse(&handle);
    step(&mut device); // the first read (1600 DPI, 1000 Hz) arms the restore
    // From here the mouse accepts the restore.
    {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(dpi_set_tx(1200), dpi_set_tx(1200));
        mock.replies.insert(dpi_get_tx(), dpi_get_reply(1200));
        mock.replies.insert(rate_set_tx(2), rate_set_tx(2));
        mock.replies
            .insert(rate_get_tx(), long(&[0x11, 0x01, 0x0b, 0x1b, 2]));
    }
    device.take_dirty();
    (device, handle)
}

fn restore_with(
    device: &mut Device<MockLink>,
    settings: &Settings,
    prefs: &DevicePrefs,
    now: Instant,
) -> Vec<String> {
    restore_last_mode_if_enabled(device, settings, |_| Some(prefs.clone()), now).unwrap()
}

fn settings_for_restore(allow: bool, restore: bool) -> Settings {
    Settings {
        allow_device_writes: allow,
        restore_last_mode: restore,
        ..Settings::default()
    }
}

#[test]
fn start_with_mode_host_restores_app_control_with_these_frames() {
    let (mut device, handle) = start_device();
    let from = handle.borrow().sent.len();
    let lines = restore_with(
        &mut device,
        &settings_for_restore(true, true),
        &host_prefs(),
        Instant::now(),
    );
    assert_eq!(
        hex_frames(&handle, from),
        [
            // enter: mode read, SetOnboardMode host, read back
            "1101092b00000000000000000000000000000000",
            "1101091b02000000000000000000000000000000",
            "1101092b00000000000000000000000000000000",
            // the button spy
            "11010a1b00000000000000000000000000000000",
            // DPI 1200: mode check, list, set, read back
            "1101092b00000000000000000000000000000000",
            "1101071b00000000000000000000000000000000",
            "1101073b0004b000000000000000000000000000",
            "1101072b00000000000000000000000000000000",
            // report rate 500 Hz (2 ms): mode check, list, set, read back
            "1101092b00000000000000000000000000000000",
            "11010b0b00000000000000000000000000000000",
            "11010b2b02000000000000000000000000000000",
            "11010b1b00000000000000000000000000000000",
        ],
        "{lines:?}"
    );
    assert!(device.host_by_us());
    assert!(device.spy_active());
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1200);
    assert_eq!(
        device.detail().unwrap().report_rate.unwrap().current_hz,
        500
    );
    // One attempt per connect event.
    let from = handle.borrow().sent.len();
    assert!(
        restore_with(
            &mut device,
            &settings_for_restore(true, true),
            &host_prefs(),
            Instant::now() + Duration::from_secs(60)
        )
        .is_empty()
    );
    assert!(hex_frames(&handle, from).is_empty());
}

/// A mouse that is already in host mode when the app starts: fn2 reads host,
/// fn4 reads 0000. Profile 3 has levels 400..3200.
fn host_at_start_device() -> (Device<MockLink>, Handle) {
    let (mut device, handle) = mock_device();
    {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(hex(FN2_TX), mode_reply(2));
        mock.replies
            .insert(fn4_tx(), long(&[0x11, 0x01, IDX_ONBOARD, 0x4b]));
    }
    step(&mut device);
    let snap = device.snapshot.as_mut().unwrap();
    for entry in &mut snap.onboard.directory.entries {
        entry.enabled |= entry.sector == 3;
    }
    let p3 = snap
        .onboard
        .profiles
        .iter_mut()
        .find(|p| p.sector == "0x0003")
        .and_then(|p| p.profile.as_mut())
        .unwrap();
    p3.dpi_levels = [400, 800, 1200, 1600, 3200];
    device.take_dirty();
    (device, handle)
}

#[test]
fn a_mouse_already_in_host_at_start_gets_its_profile_from_the_prefs() {
    let (mut device, handle) = host_at_start_device();
    assert_eq!(device.active_sector(), Some(0));
    let prefs = DevicePrefs {
        dpi: Some(1600),
        profile: Some(3),
        ..host_prefs()
    };
    let prefs = DevicePrefs {
        report_rate_hz: None,
        ..prefs
    };
    restore_with(
        &mut device,
        &settings_for_restore(true, true),
        &prefs,
        Instant::now(),
    );
    assert!(device.host_by_us());
    assert_eq!(device.remembered_profile, Some(3));
    assert_eq!(onboard_of(&device).active_profile, Some(3));
    // DPI up: 1600 -> 3200, the next level of profile 3.
    press(&mut device, &handle, &[0x0002]);
    assert_eq!(device.dpi_target, Some(3200));
}

#[test]
fn a_stored_profile_without_a_parsed_sector_is_ignored() {
    let (mut device, _handle) = host_at_start_device();
    for profile in [Some(9), Some(0), Some(0x0101)] {
        let prefs = DevicePrefs {
            profile,
            dpi: None,
            report_rate_hz: None,
            ..host_prefs()
        };
        device.last_restore = None;
        restore_with(
            &mut device,
            &settings_for_restore(true, true),
            &prefs,
            Instant::now(),
        );
        assert_eq!(device.remembered_profile, None, "{profile:?}");
    }
}

#[test]
fn in_app_control_a_confirm_read_of_0000_keeps_the_remembered_profile() {
    let (mut device, handle) = spy_device();
    assert_eq!(onboard_of(&device).active_profile, Some(2));
    {
        let mut mock = handle.borrow_mut();
        mock.replies
            .insert(fn4_tx(), long(&[0x11, 0x01, IDX_ONBOARD, 0x4b]));
        mock.replies.insert(dpi_get_tx(), dpi_get_reply(1600));
    }
    device.confirm_pending = true;
    device.confirm_read().unwrap();
    assert_eq!(onboard_of(&device).active_profile, Some(2));
    // Back in onboard mode the read is trusted as it is.
    device.snapshot.as_mut().unwrap().onboard.mode_raw_hex = Some("01000000".into());
    device.set_host_by_us(false);
    device.confirm_pending = true;
    device.confirm_read().unwrap();
    assert_eq!(onboard_of(&device).active_profile, None);
}

#[test]
fn entering_stores_the_profile_in_the_prefs_file() {
    let (dir, path) = prefs_file(OnboardMode::Onboard);
    let (mut device, handle) = read_device();
    onboard_mouse(&handle);
    enter_and_store(&mut device, Some(&path), ID, true, far_deadline()).expect("enter");
    let prefs = settings::load_prefs(&path).unwrap();
    assert_eq!(prefs.mode, OnboardMode::Host);
    assert_eq!(prefs.profile, Some(2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_is_sent_when_restore_is_off_writes_are_off_or_the_mode_is_onboard() {
    let onboard_prefs = DevicePrefs {
        mode: OnboardMode::Onboard,
        ..host_prefs()
    };
    for (settings, prefs, why) in [
        (
            settings_for_restore(true, false),
            host_prefs(),
            "restore off",
        ),
        (
            settings_for_restore(false, true),
            host_prefs(),
            "writes off",
        ),
        (settings_for_restore(false, false), host_prefs(), "both off"),
        (
            settings_for_restore(true, true),
            onboard_prefs,
            "mode onboard",
        ),
    ] {
        let (mut device, handle) = start_device();
        let from = handle.borrow().sent.len();
        let lines = restore_with(&mut device, &settings, &prefs, Instant::now());
        assert!(lines.is_empty(), "{why}: {lines:?}");
        assert!(hex_frames(&handle, from).is_empty(), "{why}: frames sent");
        assert!(!device.host_by_us(), "{why}");
        assert!(!device.take_restore_due(), "{why}: the attempt is consumed");
    }
    // No stored prefs at all: nothing either.
    let (mut device, handle) = start_device();
    let from = handle.borrow().sent.len();
    let lines = restore_last_mode_if_enabled(
        &mut device,
        &settings_for_restore(true, true),
        |_| None,
        Instant::now(),
    )
    .unwrap();
    assert!(lines.is_empty());
    assert!(hex_frames(&handle, from).is_empty());
}

#[test]
fn a_link_up_with_mode_host_enters_again_once() {
    let (mut device, handle) = start_device();
    let settings = settings_for_restore(true, true);
    let enters = |handle: &Handle| {
        handle
            .borrow()
            .sent
            .iter()
            .filter(|f| f.as_slice() == mode_set_tx(2).as_slice())
            .count()
    };
    restore_with(&mut device, &settings, &host_prefs(), Instant::now());
    assert_eq!(enters(&handle), 1);

    // The mouse is switched off and on and comes back onboard (it forgot host).
    {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(hex(FN2_TX), mode_reply(1));
        mock.replies.insert(dpi_get_tx(), dpi_get_reply(1600));
        mock.replies
            .insert(rate_get_tx(), long(&[0x11, 0x01, 0x0b, 0x1b, 1]));
    }
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    step(&mut device);
    handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
    step(&mut device);
    step(&mut device);
    assert!(!device.host_by_us(), "the mouse reads onboard again");
    // The mouse takes the restore again.
    {
        let mut mock = handle.borrow_mut();
        mock.replies.insert(dpi_get_tx(), dpi_get_reply(1200));
        mock.replies
            .insert(rate_get_tx(), long(&[0x11, 0x01, 0x0b, 0x1b, 2]));
    }
    let later = Instant::now() + Duration::from_secs(60);
    let lines = restore_with(&mut device, &settings, &host_prefs(), later);
    assert_eq!(enters(&handle), 2, "entered again: {lines:?}");
    assert!(device.host_by_us());
    assert!(device.spy_active());
    assert_eq!(device.detail().unwrap().dpi.unwrap().current, 1200);

    // Once per link-up: asking again without a new connect does nothing.
    let from = handle.borrow().sent.len();
    assert!(restore_with(&mut device, &settings, &host_prefs(), later).is_empty());
    assert!(hex_frames(&handle, from).is_empty());
}

#[test]
fn a_link_up_that_keeps_host_only_reapplies_the_values_and_keeps_the_spy() {
    let (mut device, handle) = start_device();
    let settings = settings_for_restore(true, true);
    restore_with(&mut device, &settings, &host_prefs(), Instant::now());
    // The DPI went back to 1600 over the power cycle, host mode survived.
    handle
        .borrow_mut()
        .replies
        .insert(dpi_get_tx(), dpi_get_reply(1600));
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    step(&mut device);
    handle.borrow_mut().inbox.push_back(hex("10014111a29f40"));
    step(&mut device);
    step(&mut device);
    assert!(device.host_by_us());
    assert!(
        device.spy_active(),
        "the spy was started again by the link-up"
    );
    handle
        .borrow_mut()
        .replies
        .insert(dpi_get_tx(), dpi_get_reply(1200));
    let from = handle.borrow().sent.len();
    restore_with(
        &mut device,
        &settings,
        &host_prefs(),
        Instant::now() + Duration::from_secs(60),
    );
    let sent = hex_frames(&handle, from);
    assert!(
        !sent.iter().any(|f| f.starts_with("1101091b")),
        "no switch frame: {sent:?}"
    );
    assert!(
        sent.iter().any(|f| f.starts_with("1101073b0004b0")),
        "{sent:?}"
    );
}

#[test]
fn shutdown_returns_to_onboard_but_keeps_the_stored_mode_host() {
    let dir = std::env::temp_dir().join(format!("myhub-mode-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = settings::prefs_path(&dir, ID).unwrap();
    // The user entered app control: the mode is stored.
    update_prefs(&path, |prefs| prefs.mode = OnboardMode::Host);
    assert_eq!(settings::load_prefs(&path).unwrap().mode, OnboardMode::Host);

    let (mut device, handle) = start_device();
    device.enter_temporary(ID, true, None).expect("enter");
    device = device.with_shutdown(Arc::new(AtomicBool::new(true)));
    let from = handle.borrow().sent.len();
    // What shutdown (and quit, and writes off) does: return, spy stop first.
    device.return_to_onboard().unwrap();
    let sent = hex_frames(&handle, from);
    assert!(sent[0].starts_with("11010a2b"), "{sent:?}");
    assert!(!device.host_by_us());
    assert_eq!(mode_of(&device), Some(OnboardMode::Onboard));

    // The stored mode is untouched, and so are the values.
    let prefs = settings::load_prefs(&path).unwrap();
    assert_eq!(prefs.mode, OnboardMode::Host);

    // Only an explicit leave (store_mode path) changes it.
    update_prefs(&path, |prefs| prefs.mode = OnboardMode::Onboard);
    assert_eq!(
        settings::load_prefs(&path).unwrap().mode,
        OnboardMode::Onboard
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------ mode storing, allow flag

fn prefs_file(mode: OnboardMode) -> (PathBuf, PathBuf) {
    let dir = temp_dir();
    let path = settings::prefs_path(&dir, ID).unwrap();
    update_prefs(&path, |prefs| prefs.mode = mode);
    (dir, path)
}

fn far_deadline() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

#[test]
fn leave_stores_onboard_before_any_frame_whatever_the_outcome() {
    // Unreachable mouse: the leave fails, the mode is stored anyway.
    let (dir, path) = prefs_file(OnboardMode::Host);
    let (mut device, handle) = read_device();
    handle.borrow_mut().inbox.push_back(hex("10014111629f40"));
    step(&mut device);
    let from = handle.borrow().sent.len();
    let result = leave_and_store(&mut device, Some(&path), ID, true, far_deadline());
    assert_eq!(refused(result), "disconnected");
    assert!(hex_frames(&handle, from).is_empty());
    assert_eq!(
        settings::load_prefs(&path).unwrap().mode,
        OnboardMode::Onboard
    );
    let _ = std::fs::remove_dir_all(&dir);

    // Writes off and no mark: refused, still stored.
    let (dir, path) = prefs_file(OnboardMode::Host);
    let (mut device, _handle) = read_device();
    let result = leave_and_store(&mut device, Some(&path), ID, false, far_deadline());
    assert_eq!(refused(result), "writesDisabled");
    assert_eq!(
        settings::load_prefs(&path).unwrap().mode,
        OnboardMode::Onboard
    );
    let _ = std::fs::remove_dir_all(&dir);

    // The frames of a leave that fails mid-way: the mode was stored first.
    let (dir, path) = prefs_file(OnboardMode::Host);
    let (mut device, handle) = spy_device();
    handle.borrow_mut().silent.push(mode_set_tx(1));
    let result = leave_and_store(&mut device, Some(&path), ID, true, far_deadline());
    assert!(result.is_err());
    assert_eq!(
        settings::load_prefs(&path).unwrap().mode,
        OnboardMode::Onboard
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn enter_stores_host_only_when_it_worked() {
    let (dir, path) = prefs_file(OnboardMode::Onboard);
    let (mut device, handle) = read_device();
    onboard_mouse(&handle);
    // Writes off: refused, nothing stored.
    assert_eq!(
        refused(enter_and_store(
            &mut device,
            Some(&path),
            ID,
            false,
            far_deadline()
        )),
        "writesDisabled"
    );
    assert_eq!(
        settings::load_prefs(&path).unwrap().mode,
        OnboardMode::Onboard
    );
    // Allowed: stored.
    enter_and_store(&mut device, Some(&path), ID, true, far_deadline()).expect("enter");
    assert_eq!(settings::load_prefs(&path).unwrap().mode, OnboardMode::Host);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_save_that_returned_the_mouse_from_host_stores_onboard() {
    let root = temp_dir();
    let (dir, path) = prefs_file(OnboardMode::Host);
    let (mut device, _handle, _sim) = save_device(3, 2, true);
    device.set_host_by_us(true);
    let (result, _) =
        device.save_profile_attempt(ID, &request(), true, far(), Some(&root), 1_760_520_605);
    assert!(result.is_ok());
    assert!(device.returned_from_host);
    record_save_mode(&mut device, Some(&path));
    assert_eq!(
        settings::load_prefs(&path).unwrap().mode,
        OnboardMode::Onboard
    );
    assert!(!device.returned_from_host, "taken once");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_save_that_did_not_come_from_host_leaves_the_stored_mode_alone() {
    let root = temp_dir();
    let (dir, path) = prefs_file(OnboardMode::Host);
    let (mut device, _handle, _sim) = save_device(3, 1, true);
    let (result, _) =
        device.save_profile_attempt(ID, &request(), true, far(), Some(&root), 1_760_520_605);
    assert!(result.is_ok());
    assert!(!device.returned_from_host);
    record_save_mode(&mut device, Some(&path));
    assert_eq!(settings::load_prefs(&path).unwrap().mode, OnboardMode::Host);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_save_after_the_return_still_stores_onboard() {
    let root = temp_dir();
    let (dir, path) = prefs_file(OnboardMode::Host);
    let (mut device, _handle, _sim) = save_device(3, 2, true);
    device.set_host_by_us(true);
    // The backup cannot be written: the save fails after the return.
    let file = root.join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    let (result, _) =
        device.save_profile_attempt(ID, &request(), true, far(), Some(&file), 1_760_520_605);
    assert_eq!(code(result), "backupFailed");
    record_save_mode(&mut device, Some(&path));
    assert_eq!(
        settings::load_prefs(&path).unwrap().mode,
        OnboardMode::Onboard
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_restore_passes_the_allow_flag_to_enter_and_to_the_writes() {
    let (mut device, handle) = start_device();
    let from = handle.borrow().sent.len();
    let lines = device
        .restore_app_control(ID, &host_prefs(), false, Instant::now())
        .unwrap();
    assert!(
        lines.iter().any(|l| l.contains("writesDisabled")),
        "{lines:?}"
    );
    assert!(hex_frames(&handle, from).is_empty(), "frames were sent");
    assert!(!device.host_by_us());

    // With writes allowed the same call enters.
    let (mut device, handle) = start_device();
    let from = handle.borrow().sent.len();
    device
        .restore_app_control(ID, &host_prefs(), true, Instant::now())
        .unwrap();
    assert!(hex_frames(&handle, from).len() > 4);
    assert!(device.host_by_us());
}
