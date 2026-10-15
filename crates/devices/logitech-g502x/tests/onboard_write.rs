//! Flash write path against a simulated flash. No device, no hidapi: the
//! session talks to an in-memory link that answers discovery and reads from the
//! recorded fixture and keeps written sectors in a map.

mod common;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use common::{FIXTURE_JSON, IDX_ONBOARD, hex, logged_pairs};
use hidpp::onboard_write::{ProfilePatch, SECTOR_LEN, write_chunks};
use hidpp::policy::PolicyError;
use hidpp::{FeatureId, crc};
use logitech_g502x::{
    ProfileSource, ReadbackCheck, RestoreStatus, SectorWriteError, WriteOutcome, restore_sector,
    write_profile_sector,
};
use myhub_transport::{HidLink, HidppSession, LinkError, SessionError};

#[derive(Default)]
struct Sim {
    base: HashMap<Vec<u8>, Vec<u8>>,
    flash: HashMap<u16, Vec<u8>>,
    inbox: VecDeque<Vec<u8>>,
    /// Every frame the session wrote.
    writes: Vec<Vec<u8>>,
    pending: Option<(u16, Vec<u8>)>,
    /// 0x8100 calls seen per function number.
    calls: HashMap<u8, usize>,
    /// `(function, 1-based call number, fault)`, each fires once.
    faults: Vec<(u8, usize, Fault)>,
    /// An incomplete write stores its partial data at EndWrite.
    commit_partial: bool,
    /// Active profile sector reported by fn4 (fn3 changes it).
    active: u16,
    /// fn3 is acknowledged but the active profile does not change.
    ignore_select: bool,
    /// fn2 IsOnboardMode reply: 1 onboard, 2 host.
    mode: u8,
    /// Number of upcoming commits that store a corrupted byte.
    corrupt_commits: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// HID++ 2.0 error reply, no effect on the flash.
    DeviceError,
    /// The call takes effect but no reply arrives.
    Silent,
    /// A HID++ 1.0 0x41 "link down" arrives instead of the reply, no effect.
    LinkDown,
}

type Handle = Rc<RefCell<Sim>>;

struct SimLink(Handle);

fn header(frame: &[u8]) -> Vec<u8> {
    frame[..4].to_vec()
}

impl HidLink for SimLink {
    fn write_report(&mut self, report: &[u8; 20]) -> Result<(), LinkError> {
        let mut sim = self.0.borrow_mut();
        sim.writes.push(report.to_vec());
        if report[2] == IDX_ONBOARD {
            let function = report[3] >> 4;
            let params = &report[4..];
            let mut ack = header(report);
            ack.resize(20, 0);
            let n = {
                let c = sim.calls.entry(function).or_insert(0);
                *c += 1;
                *c
            };
            let fault = sim
                .faults
                .iter()
                .position(|&(f, k, _)| f == function && k == n)
                .map(|i| sim.faults.remove(i).2);
            match fault {
                Some(Fault::DeviceError) => {
                    let mut err = vec![0x11, report[1], 0xFF, report[2], report[3], 0x02];
                    err.resize(20, 0);
                    sim.inbox.push_back(err);
                    return Ok(());
                }
                Some(Fault::LinkDown) => {
                    let mut down = vec![0x11, report[1], 0x41, 0x00, 0x40, 0x9f, 0x40];
                    down.resize(20, 0);
                    sim.inbox.push_back(down);
                    return Ok(());
                }
                _ => {}
            }
            let silent = fault == Some(Fault::Silent);
            let reply: Vec<u8> = match function {
                5 => {
                    let sector = u16::from_be_bytes([params[0], params[1]]);
                    let off = usize::from(u16::from_be_bytes([params[2], params[3]]));
                    let mut r = header(report);
                    r.extend_from_slice(&sim.flash[&sector][off..off + 16]);
                    r
                }
                2 => {
                    let mut r = header(report);
                    r.push(sim.mode);
                    r.resize(20, 0);
                    r
                }
                3 => {
                    if !sim.ignore_select {
                        sim.active = u16::from_be_bytes([params[0], params[1]]);
                    }
                    ack
                }
                4 => {
                    let mut r = header(report);
                    r.extend_from_slice(&sim.active.to_be_bytes());
                    r.resize(20, 0);
                    r
                }
                6 => {
                    let sector = u16::from_be_bytes([params[0], params[1]]);
                    sim.pending = Some((sector, Vec::new()));
                    ack
                }
                7 => {
                    if let Some((_, buf)) = sim.pending.as_mut() {
                        buf.extend_from_slice(params);
                    }
                    ack
                }
                8 => {
                    if let Some((sector, buf)) = sim.pending.take() {
                        if buf.len() >= 256 {
                            let mut stored = buf[..SECTOR_LEN].to_vec();
                            if sim.corrupt_commits > 0 {
                                sim.corrupt_commits -= 1;
                                stored[0x40] ^= 0x01;
                            }
                            sim.flash.insert(sector, stored);
                        } else if sim.commit_partial {
                            let mut stored = sim.flash[&sector].clone();
                            stored[..buf.len().min(SECTOR_LEN)]
                                .copy_from_slice(&buf[..buf.len().min(SECTOR_LEN)]);
                            sim.flash.insert(sector, stored);
                        }
                    }
                    ack
                }
                _ => sim
                    .base
                    .get(report.as_slice())
                    .unwrap_or_else(|| panic!("unexpected 0x8100 fn{function}"))
                    .clone(),
            };
            if !silent {
                sim.inbox.push_back(reply);
            }
        } else {
            let reply = sim
                .base
                .get(report.as_slice())
                .unwrap_or_else(|| panic!("unexpected TX {report:02x?}"))
                .clone();
            sim.inbox.push_back(reply);
        }
        Ok(())
    }

    fn short_reports_available(&self) -> bool {
        true
    }

    fn read_report(&mut self, buf: &mut [u8; 64], _timeout_ms: i32) -> Result<usize, LinkError> {
        let next = self.0.borrow_mut().inbox.pop_front();
        match next {
            Some(frame) => {
                buf[..frame.len()].copy_from_slice(&frame);
                Ok(frame.len())
            }
            None => {
                std::thread::sleep(Duration::from_millis(1));
                Ok(0)
            }
        }
    }
}

fn session() -> (HidppSession<SimLink>, Handle) {
    let mut sim = Sim {
        active: 2,
        mode: 1,
        ..Sim::default()
    };
    for (tx, rx) in logged_pairs() {
        sim.base.insert(tx, rx);
    }
    let json: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
    for (key, value) in json["sectors"].as_object().unwrap() {
        let sector = u16::from_str_radix(key.trim_start_matches("0x"), 16).unwrap();
        sim.flash.insert(sector, hex(value.as_str().unwrap()));
    }
    let handle: Handle = Rc::new(RefCell::new(sim));
    let mut session = logitech_g502x::new_session(SimLink(handle.clone()));
    session.set_reply_timeout(Duration::from_millis(100));
    session.discover().expect("discover");
    (session, handle)
}

/// `(function, params)` of every 0x8100 fn6/7/8 frame sent so far.
fn write_frames(handle: &Handle) -> Vec<(u8, [u8; 16])> {
    handle
        .borrow()
        .writes
        .iter()
        .filter(|f| f[2] == IDX_ONBOARD && (6..=8).contains(&(f[3] >> 4)))
        .map(|f| (f[3] >> 4, f[4..].try_into().unwrap()))
        .collect()
}

fn sector(handle: &Handle, n: u16) -> Vec<u8> {
    handle.borrow().flash[&n].clone()
}

fn rate_patch() -> ProfilePatch {
    ProfilePatch {
        report_rate_interval_ms: Some(4),
        default_dpi_index: Some(1),
        shift_dpi_index: Some(3),
        dpi_levels: Some([400, 800, 1200, 1600, 3200]),
    }
}

type Saved = Rc<RefCell<Vec<(u16, Vec<u8>, usize)>>>;

/// A backup callback that records (sector, bytes, write frames seen so far).
fn recorder(handle: &Handle, saved: &Saved) -> impl FnMut(u16, &[u8; 255]) -> Result<(), String> {
    let handle = handle.clone();
    let saved = saved.clone();
    move |sector, bytes| {
        saved
            .borrow_mut()
            .push((sector, bytes.to_vec(), write_frames(&handle).len()));
        Ok(())
    }
}

#[test]
fn full_sequence_and_bytes() {
    let (mut s, h) = session();
    let original = sector(&h, 3);
    let saved: Saved = Default::default();
    let mut backup = recorder(&h, &saved);

    let report = write_profile_sector(
        &mut s,
        3,
        &rate_patch(),
        ProfileSource::Same,
        &mut backup,
        false,
    )
    .unwrap();

    assert_eq!(report.outcome, WriteOutcome::Written);
    assert_eq!(report.read_bytes, original);
    // The backup got the raw bytes before any write frame.
    assert_eq!(saved.borrow().as_slice(), &[(3, original.clone(), 0)]);
    // The flash now holds the image, with a valid CRC and the new values.
    let now = sector(&h, 3);
    assert_eq!(now, report.image);
    assert_ne!(now, original);
    assert!(crc::sector_crc_ok(&now));

    // fn6, 16 x fn7, fn8, in that order and with those bytes.
    let frames = write_frames(&h);
    assert_eq!(frames.len(), 18);
    assert_eq!(frames[0].0, 6);
    assert_eq!(frames[0].1[..6], [0x00, 0x03, 0x00, 0x00, 0x00, 0xFF]);
    assert!(frames[0].1[6..].iter().all(|&b| b == 0));
    let image: [u8; 255] = report.image.as_slice().try_into().unwrap();
    for (i, chunk) in write_chunks(&image).iter().enumerate() {
        assert_eq!(frames[1 + i], (7, *chunk), "chunk {i}");
    }
    assert_eq!(frames[17], (8, [0u8; 16]));
    // Replies are logged as hex.
    assert!(report.log.begin.is_some() && report.log.end.is_some());
    assert_eq!(report.log.data.len(), 16);
    // Only sector 3 changed.
    for n in [0u16, 1, 2, 4, 5, 0x101] {
        let json: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
        let want = json["sectors"][format!("0x{n:04x}")].as_str().unwrap();
        assert_eq!(sector(&h, n), hex(want), "sector {n:#06x}");
    }
}

#[test]
fn backup_failure_sends_no_write_frame() {
    let (mut s, h) = session();
    let mut backup = |_: u16, _: &[u8; 255]| Err("disk full".to_string());
    let err = write_profile_sector(
        &mut s,
        3,
        &rate_patch(),
        ProfileSource::Same,
        &mut backup,
        false,
    )
    .unwrap_err();
    assert_eq!(err, SectorWriteError::Backup("disk full".into()));
    assert!(write_frames(&h).is_empty());
}

#[test]
fn unchanged_image_sends_no_write_frame_unless_forced() {
    let (mut s, h) = session();
    let original = sector(&h, 3);
    let saved: Saved = Default::default();
    let mut backup = recorder(&h, &saved);

    let report = write_profile_sector(
        &mut s,
        3,
        &ProfilePatch::default(),
        ProfileSource::Same,
        &mut backup,
        false,
    )
    .unwrap();
    assert_eq!(report.outcome, WriteOutcome::Unchanged);
    assert_eq!(report.image, original);
    assert!(write_frames(&h).is_empty());

    let report = write_profile_sector(
        &mut s,
        3,
        &ProfilePatch::default(),
        ProfileSource::Same,
        &mut backup,
        true,
    )
    .unwrap();
    assert_eq!(report.outcome, WriteOutcome::Written);
    assert_eq!(write_frames(&h).len(), 18);
    assert_eq!(sector(&h, 3), original);
}

#[test]
fn patch_refusal_sends_no_write_frame() {
    let (mut s, h) = session();
    let mut backup = |_: u16, _: &[u8; 255]| Ok(());
    let patch = ProfilePatch {
        dpi_levels: Some([800, 800, 801, 800, 800]),
        ..Default::default()
    };
    let err = write_profile_sector(&mut s, 3, &patch, ProfileSource::Same, &mut backup, false)
        .unwrap_err();
    assert!(matches!(err, SectorWriteError::Patch(_)), "{err:?}");
    assert!(write_frames(&h).is_empty());
}

#[test]
fn fn7_error_still_sends_fn8_and_restores_a_partly_written_sector() {
    let (mut s, h) = session();
    let original = sector(&h, 3);
    {
        // The device keeps the two chunks it got at EndWrite.
        let mut sim = h.borrow_mut();
        sim.faults.push((7, 3, Fault::DeviceError));
        sim.commit_partial = true;
    }
    let mut backup = |_: u16, _: &[u8; 255]| Ok(());

    let err = write_profile_sector(
        &mut s,
        3,
        &rate_patch(),
        ProfileSource::Same,
        &mut backup,
        false,
    )
    .unwrap_err();
    let SectorWriteError::WriteFailed { cause, detail } = err else {
        panic!("{err:?}");
    };
    assert!(
        matches!(cause, SessionError::Device { code: 2, .. }),
        "{cause:?}"
    );
    assert_eq!(detail.readback, ReadbackCheck::Differs);
    assert!(matches!(detail.restore, RestoreStatus::Restored(_)));
    assert_eq!(detail.log.data.len(), 2);

    let fns: Vec<u8> = write_frames(&h).iter().map(|f| f.0).collect();
    // First attempt: 6, 7, 7, 7 (error), 8. Restore: 6, 16 x 7, 8.
    let mut want = vec![6, 7, 7, 7, 8, 6];
    want.extend([7u8; 16]);
    want.push(8);
    assert_eq!(fns, want);
    // The restore wrote the original bytes.
    let frames = write_frames(&h);
    let original: [u8; 255] = original.as_slice().try_into().unwrap();
    for (i, chunk) in write_chunks(&original).iter().enumerate() {
        assert_eq!(frames[6 + i], (7, *chunk));
    }
    assert_eq!(sector(&h, 3), original);
}

#[test]
fn read_back_mismatch_restores() {
    let (mut s, h) = session();
    let original = sector(&h, 3);
    h.borrow_mut().corrupt_commits = 1;
    let mut backup = |_: u16, _: &[u8; 255]| Ok(());

    let err = write_profile_sector(
        &mut s,
        3,
        &rate_patch(),
        ProfileSource::Same,
        &mut backup,
        false,
    )
    .unwrap_err();
    let SectorWriteError::VerifyFailed {
        first_diff,
        crc_ok,
        detail,
    } = err
    else {
        panic!("{err:?}");
    };
    assert_eq!(first_diff, Some(0x40));
    assert!(!crc_ok);
    assert_eq!(detail.readback, ReadbackCheck::Differs);
    assert!(matches!(detail.restore, RestoreStatus::Restored(_)));
    assert_eq!(write_frames(&h).len(), 36);
    assert_eq!(sector(&h, 3), original);
}

#[test]
fn failed_restore_is_reported() {
    let (mut s, h) = session();
    h.borrow_mut().corrupt_commits = 2;
    let mut backup = |_: u16, _: &[u8; 255]| Ok(());
    let err = write_profile_sector(
        &mut s,
        3,
        &rate_patch(),
        ProfileSource::Same,
        &mut backup,
        false,
    )
    .unwrap_err();
    let SectorWriteError::VerifyFailed { detail, .. } = &err else {
        panic!("{err:?}");
    };
    assert!(matches!(detail.restore, RestoreStatus::Failed(..)));
    assert!(err.to_string().contains("RESTORE FAILED"));
    // One restore attempt only.
    assert_eq!(write_frames(&h).len(), 36);
}

#[test]
fn non_allowlisted_and_rom_sectors_are_refused_before_any_frame() {
    let (mut s, h) = session();
    let before = h.borrow().writes.len();
    let mut backup = |_: u16, _: &[u8; 255]| Ok(());
    let image = sector(&h, 3);
    for n in [0x0001u16, 0x0002, 0x0006, 0x0100, 0x0101, 0x0105, 0xFFFF] {
        assert_eq!(
            write_profile_sector(
                &mut s,
                n,
                &rate_patch(),
                ProfileSource::Same,
                &mut backup,
                false
            )
            .unwrap_err(),
            SectorWriteError::SectorNotAllowed(n)
        );
        assert_eq!(
            restore_sector(&mut s, n, &image).unwrap_err(),
            SectorWriteError::SectorNotAllowed(n)
        );
    }
    assert_eq!(h.borrow().writes.len(), before, "a frame was sent");
}

#[test]
fn format_other_than_3_is_refused_before_any_write() {
    let (mut s, h) = session();
    // Change the recorded GetInfo reply: profile format byte (payload[1]).
    {
        let mut sim = h.borrow_mut();
        let key = sim
            .base
            .keys()
            .find(|k| k[2] == IDX_ONBOARD && k[3] >> 4 == 0)
            .expect("info request in the fixture")
            .clone();
        sim.base.get_mut(&key).unwrap()[5] = 4;
    }
    let before = h.borrow().writes.len();
    let mut backup = |_: u16, _: &[u8; 255]| Ok(());
    let image = sector(&h, 3);
    assert_eq!(
        write_profile_sector(
            &mut s,
            3,
            &rate_patch(),
            ProfileSource::Same,
            &mut backup,
            false
        )
        .unwrap_err(),
        SectorWriteError::UnsupportedFormat(4)
    );
    assert_eq!(
        restore_sector(&mut s, 3, &image).unwrap_err(),
        SectorWriteError::UnsupportedFormat(4)
    );
    // Only the info reads were sent.
    assert_eq!(h.borrow().writes.len(), before + 2);
    assert!(write_frames(&h).is_empty());
}

#[test]
fn restore_accepts_only_valid_images() {
    let (mut s, h) = session();
    let good = sector(&h, 3);

    let mut short = good.clone();
    short.pop();
    assert!(matches!(
        restore_sector(&mut s, 3, &short),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    let mut bad_crc = good.clone();
    bad_crc[0x10] ^= 1;
    assert!(matches!(
        restore_sector(&mut s, 3, &bad_crc),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    assert!(write_frames(&h).is_empty());

    // A valid image of another sector goes through the full sequence.
    let log = restore_sector(&mut s, 4, &good).unwrap();
    assert_eq!(log.data.len(), 16);
    assert_eq!(write_frames(&h).len(), 18);
    assert_eq!(sector(&h, 4), good);
}

#[test]
fn read_and_volatile_paths_cannot_send_flash_writes() {
    let (mut s, h) = session();
    let before = h.borrow().writes.len();
    let mut begin = [0u8; 16];
    begin[1] = 3;
    begin[5] = 255;
    for function in [6u8, 7, 8] {
        let err = s
            .request_feature(FeatureId::ONBOARD_PROFILES, function, begin)
            .unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Policy(PolicyError::NotAllowlisted { .. })
            ),
            "read fn {function}: {err:?}"
        );
        let err = s
            .request_volatile(FeatureId::ONBOARD_PROFILES, function, begin)
            .unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Policy(PolicyError::NotAllowlisted { .. })
            ),
            "volatile fn {function}: {err:?}"
        );
        let err = s.request(IDX_ONBOARD, function, begin).unwrap_err();
        assert!(matches!(
            err,
            SessionError::Policy(PolicyError::NotAllowlisted { .. })
        ));
    }
    assert_eq!(h.borrow().writes.len(), before, "a frame was sent");
}

#[test]
fn write_sector_sends_exactly_the_sealed_image() {
    use hidpp::onboard::OnboardInfo;
    use hidpp::onboard_write::{restore_profile_image, write_chunks};
    let (mut s, h) = session();
    let info = {
        let json: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
        OnboardInfo::parse(&hex(json["8100_fn0_info"].as_str().unwrap())).unwrap()
    };
    let bytes = sector(&h, 3);
    let image = restore_profile_image(3, &bytes, &info).unwrap();
    let before = h.borrow().writes.len();
    let (log, result) = s.write_sector(&image);
    assert_eq!(result, Ok(()));
    assert!(log.begin.is_some() && log.end.is_some());
    assert_eq!(log.data.len(), 16);
    // fn6 params, 16 chunks, fn8: nothing else went out.
    let frames = write_frames(&h);
    assert_eq!(frames.len(), 18);
    assert_eq!(h.borrow().writes.len(), before + 18);
    assert_eq!(frames[0].1[..6], [0, 3, 0, 0, 0, 0xFF]);
    for (i, chunk) in write_chunks(image.bytes()).iter().enumerate() {
        assert_eq!(frames[1 + i], (7, *chunk));
    }
    // Images for ROM sectors and other sectors cannot even be made.
    for target in [1u16, 2, 6, 0x0101, 0x0105] {
        assert!(
            restore_profile_image(target, &bytes, &info).is_err(),
            "{target}"
        );
    }
}

#[test]
fn flash_timeout_applies_to_fn6_and_fn8_only_and_the_session_timeout_returns() {
    use hidpp::onboard::OnboardInfo;
    use hidpp::onboard_write::restore_profile_image;
    use std::time::Instant;
    let (mut s, h) = session();
    let info = {
        let json: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
        OnboardInfo::parse(&hex(json["8100_fn0_info"].as_str().unwrap())).unwrap()
    };
    let image = restore_profile_image(3, &sector(&h, 3), &info).unwrap();
    h.borrow_mut().faults.push((8, 1, Fault::Silent));
    let started = Instant::now();
    let (log, result) = s.write_sector(&image);
    // fn8 waited for the flash timeout (3 s), not the session's 100 ms.
    assert_eq!(result, Err(SessionError::Timeout));
    assert!(started.elapsed() >= Duration::from_millis(2500));
    assert_eq!(log.end, None);
    // Afterwards the session timeout (100 ms) is back.
    let next = h.borrow().calls.get(&0).copied().unwrap_or(0) + 1;
    h.borrow_mut().faults.push((0, next, Fault::Silent));
    let started = Instant::now();
    let err = s
        .request_feature(FeatureId::ONBOARD_PROFILES, 0, [0; 16])
        .unwrap_err();
    assert_eq!(err, SessionError::Timeout);
    assert!(started.elapsed() < Duration::from_millis(1500));
}

/// Runs a failing write with `faults` injected and returns the error.
fn failing_write(faults: &[(u8, usize, Fault)]) -> (SectorWriteError, Handle, Vec<u8>) {
    let (mut s, h) = session();
    let original = sector(&h, 3);
    h.borrow_mut().faults.extend_from_slice(faults);
    let mut backup = |_: u16, _: &[u8; 255]| Ok(());
    let err = write_profile_sector(
        &mut s,
        3,
        &rate_patch(),
        ProfileSource::Same,
        &mut backup,
        false,
    )
    .unwrap_err();
    (err, h, original)
}

fn fns(h: &Handle) -> Vec<u8> {
    write_frames(h).iter().map(|f| f.0).collect()
}

fn detail_of(err: &SectorWriteError) -> &logitech_g502x::FailureDetail {
    match err {
        SectorWriteError::WriteFailed { detail, .. }
        | SectorWriteError::VerifyFailed { detail, .. } => detail,
        other => panic!("{other:?}"),
    }
}

#[test]
fn fn6_device_error_sends_fn8_and_needs_no_restore() {
    let (err, h, original) = failing_write(&[(6, 1, Fault::DeviceError)]);
    let SectorWriteError::WriteFailed { cause, detail } = &err else {
        panic!("{err:?}");
    };
    assert!(matches!(cause, SessionError::Device { code: 2, .. }));
    assert_eq!(detail.readback, ReadbackCheck::Identical);
    assert_eq!(detail.restore, RestoreStatus::NotNeeded);
    // fn6 failed, no fn7, fn8 still sent, no restore sequence.
    assert_eq!(fns(&h), vec![6, 8]);
    assert_eq!(sector(&h, 3), original);
}

#[test]
fn fn6_timeout_sends_fn8_and_needs_no_restore() {
    let (err, h, original) = failing_write(&[(6, 1, Fault::Silent)]);
    let SectorWriteError::WriteFailed { cause, detail } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(*cause, SessionError::Timeout);
    assert_eq!(detail.restore, RestoreStatus::NotNeeded);
    assert_eq!(fns(&h), vec![6, 8]);
    assert_eq!(sector(&h, 3), original);
}

#[test]
fn fn8_timeout_with_a_committed_image_restores_the_original() {
    // The commit happened but its reply never came: the sector differs from
    // the original, so the backup is written back.
    let (err, h, original) = failing_write(&[(8, 1, Fault::Silent)]);
    let SectorWriteError::WriteFailed { cause, detail } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(*cause, SessionError::Timeout);
    assert_eq!(detail.readback, ReadbackCheck::Differs);
    assert!(matches!(detail.restore, RestoreStatus::Restored(_)));
    assert_eq!(detail.log.data.len(), 16);
    assert_eq!(detail.log.end, None);
    let mut want = vec![6];
    want.extend([7u8; 16]);
    want.push(8);
    let mut twice = want.clone();
    twice.extend(want);
    assert_eq!(fns(&h), twice);
    assert_eq!(sector(&h, 3), original);
}

#[test]
fn verify_read_timeout_restores_when_the_sector_cannot_be_read() {
    // fn5 calls 1..=16 are the initial read; 17 is the verify read, 18 the
    // check after the failure. Both time out, so the restore is attempted.
    let (err, h, original) = failing_write(&[(5, 17, Fault::Silent), (5, 18, Fault::Silent)]);
    let SectorWriteError::WriteFailed { cause, detail } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(*cause, SessionError::Timeout);
    assert!(matches!(detail.readback, ReadbackCheck::Unreadable(_)));
    assert!(matches!(detail.restore, RestoreStatus::Restored(_)));
    assert_eq!(fns(&h).len(), 36);
    assert_eq!(sector(&h, 3), original);
}

#[test]
fn verify_read_timeout_alone_still_restores_a_differing_sector() {
    // Only the verify read fails; the check afterwards reads the image, which
    // differs from the original, so the original is written back.
    let (err, h, original) = failing_write(&[(5, 17, Fault::Silent)]);
    let detail = detail_of(&err);
    assert_eq!(detail.readback, ReadbackCheck::Differs);
    assert!(matches!(detail.restore, RestoreStatus::Restored(_)));
    assert_eq!(sector(&h, 3), original);
}

#[test]
fn lost_link_mid_sequence_sends_no_fn8_and_no_restore() {
    // A 0x41 instead of the third fn7 reply.
    let (err, h, _) = failing_write(&[(7, 3, Fault::LinkDown)]);
    let SectorWriteError::WriteFailed { cause, detail } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(*cause, SessionError::NeedsRediscovery);
    assert_eq!(detail.readback, ReadbackCheck::Skipped);
    assert_eq!(detail.restore, RestoreStatus::NotAttempted);
    assert_eq!(fns(&h), vec![6, 7, 7, 7]);
    // No fn5 read-back frame either: only the 16 initial reads.
    assert_eq!(h.borrow().calls[&5], 16);
}

#[test]
fn lost_link_at_fn8_or_verify_read_sends_no_restore() {
    let (err, h, _) = failing_write(&[(8, 1, Fault::LinkDown)]);
    assert_eq!(detail_of(&err).restore, RestoreStatus::NotAttempted);
    assert_eq!(fns(&h).len(), 18);
    assert_eq!(h.borrow().calls[&5], 16);

    let (err, h, _) = failing_write(&[(5, 17, Fault::LinkDown)]);
    assert_eq!(detail_of(&err).readback, ReadbackCheck::Skipped);
    assert_eq!(detail_of(&err).restore, RestoreStatus::NotAttempted);
    assert_eq!(fns(&h).len(), 18);
    assert_eq!(h.borrow().calls[&5], 17);
}

// ---- stage 5: clone, directory, select ----

use logitech_g502x::{select_profile, write_directory};

fn nop_backup() -> impl FnMut(u16, &[u8; 255]) -> Result<(), String> {
    |_, _| Ok(())
}

/// Sector numbers of every 0x8100 fn5 request sent, in order.
fn read_sectors(h: &Handle) -> Vec<u16> {
    h.borrow()
        .writes
        .iter()
        .filter(|f| f[2] == IDX_ONBOARD && f[3] >> 4 == 5)
        .map(|f| u16::from_be_bytes([f[4], f[5]]))
        .collect()
}

fn select_frames(h: &Handle) -> Vec<Vec<u8>> {
    h.borrow()
        .writes
        .iter()
        .filter(|f| f[2] == IDX_ONBOARD && f[3] >> 4 == 3)
        .cloned()
        .collect()
}

#[test]
fn write_profile_refuses_the_directory_and_bad_sources_before_any_frame() {
    let (mut s, h) = session();
    let before = h.borrow().writes.len();
    let mut backup = nop_backup();
    for (sector, source, want) in [
        (
            0u16,
            ProfileSource::Same,
            SectorWriteError::SectorNotAllowed(0),
        ),
        (
            0,
            ProfileSource::CloneFrom(2),
            SectorWriteError::SectorNotAllowed(0),
        ),
        (
            1,
            ProfileSource::Same,
            SectorWriteError::SectorNotAllowed(1),
        ),
        (
            3,
            ProfileSource::CloneFrom(0),
            SectorWriteError::BadSource(0),
        ),
        (
            3,
            ProfileSource::CloneFrom(6),
            SectorWriteError::BadSource(6),
        ),
        (
            3,
            ProfileSource::CloneFrom(0x0101),
            SectorWriteError::BadSource(0x0101),
        ),
        (
            3,
            ProfileSource::CloneFrom(3),
            SectorWriteError::BadSource(3),
        ),
    ] {
        assert_eq!(
            write_profile_sector(&mut s, sector, &rate_patch(), source, &mut backup, true)
                .unwrap_err(),
            want
        );
    }
    assert_eq!(h.borrow().writes.len(), before, "a frame was sent");
}

#[test]
fn clone_from_copies_the_source_then_patches_it() {
    let (mut s, h) = session();
    let target_before = sector(&h, 3);
    let source = sector(&h, 2);
    let saved: Saved = Default::default();
    let mut backup = recorder(&h, &saved);

    let report = write_profile_sector(
        &mut s,
        3,
        &rate_patch(),
        ProfileSource::CloneFrom(2),
        &mut backup,
        false,
    )
    .unwrap();
    assert_eq!(report.outcome, WriteOutcome::Written);
    // The backup is of the target, not of the source, and came before any write.
    assert_eq!(saved.borrow().as_slice(), &[(3, target_before.clone(), 0)]);
    assert_eq!(report.read_bytes, target_before);
    // Reads: target first, then the source.
    let reads = read_sectors(&h);
    assert_eq!(&reads[..16], &[3u16; 16]);
    assert_eq!(&reads[16..32], &[2u16; 16]);
    // Sector 3 now holds the patched copy of sector 2; sector 2 is untouched.
    let info = {
        let json: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
        hidpp::onboard::OnboardInfo::parse(&hex(json["8100_fn0_info"].as_str().unwrap())).unwrap()
    };
    let new = hidpp::onboard::ProfileSector::parse(&sector(&h, 3), &info).unwrap();
    let old = hidpp::onboard::ProfileSector::parse(&source, &info).unwrap();
    assert!(new.crc_ok);
    assert_eq!(new.report_rate, 4);
    assert_eq!(new.dpi_levels, [400, 800, 1200, 1600, 3200]);
    assert_eq!(new.buttons, old.buttons);
    assert_eq!(new.gshift_buttons, old.gshift_buttons);
    assert_eq!(new.name, old.name);
    assert_eq!(sector(&h, 2), source);
    assert_eq!(write_frames(&h).len(), 18);
}

#[test]
fn write_directory_enables_one_entry_through_the_normal_sequence() {
    let (mut s, h) = session();
    let original = sector(&h, 0);
    let saved: Saved = Default::default();
    let mut backup = recorder(&h, &saved);

    let report = write_directory(&mut s, 3, true, &mut backup, false).unwrap();
    assert_eq!(report.outcome, WriteOutcome::Written);
    assert_eq!(saved.borrow().as_slice(), &[(0, original.clone(), 0)]);
    // Same frames as a profile write, aimed at sector 0.
    let frames = write_frames(&h);
    assert_eq!(frames.len(), 18);
    assert_eq!(frames[0].1[..6], [0, 0, 0, 0, 0, 0xFF]);
    let now = sector(&h, 0);
    assert_eq!(now, report.image);
    let diff: Vec<usize> = (0..253).filter(|&i| now[i] != original[i]).collect();
    assert_eq!(diff, vec![10]);
    assert_eq!(now[10], 1);
    assert!(crc::sector_crc_ok(&now));
    for n in 1..=5u16 {
        let json: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
        let want = json["sectors"][format!("0x{n:04x}")].as_str().unwrap();
        assert_eq!(sector(&h, n), hex(want), "sector {n}");
    }

    // Enabling again is a no-op; disabling returns the original bytes.
    let again = write_directory(&mut s, 3, true, &mut nop_backup(), false).unwrap();
    assert_eq!(again.outcome, WriteOutcome::Unchanged);
    assert_eq!(write_frames(&h).len(), 18);
    write_directory(&mut s, 3, false, &mut nop_backup(), false).unwrap();
    assert_eq!(sector(&h, 0), original);
}

#[test]
fn write_directory_refusals_send_no_write_frame() {
    let (mut s, h) = session();
    let mut backup = |_: u16, _: &[u8; 255]| -> Result<(), String> {
        panic!("the backup must not run for a refused change")
    };
    for sector_nr in [0u16, 1, 2, 6, 0x0101] {
        assert_eq!(
            write_directory(&mut s, sector_nr, true, &mut backup, false).unwrap_err(),
            SectorWriteError::SectorNotAllowed(sector_nr)
        );
    }
    // Disabling the active profile, and disabling the last enabled one.
    write_directory(&mut s, 3, true, &mut nop_backup(), false).unwrap();
    let writes_before = write_frames(&h).len();
    h.borrow_mut().active = 3;
    assert!(matches!(
        write_directory(&mut s, 3, false, &mut backup, false).unwrap_err(),
        SectorWriteError::Patch(hidpp::onboard_write::WriteError::ActiveProfileDisabled(3))
    ));
    assert_eq!(write_frames(&h).len(), writes_before);
}

#[test]
fn write_directory_failure_restores_the_directory() {
    let (mut s, h) = session();
    let original = sector(&h, 0);
    {
        let mut sim = h.borrow_mut();
        sim.faults.push((7, 2, Fault::DeviceError));
        sim.commit_partial = true;
    }
    let err = write_directory(&mut s, 3, true, &mut nop_backup(), false).unwrap_err();
    let detail = detail_of(&err);
    assert_eq!(detail.readback, ReadbackCheck::Differs);
    assert!(matches!(detail.restore, RestoreStatus::Restored(_)));
    assert_eq!(sector(&h, 0), original);
}

#[test]
fn select_profile_needs_an_enabled_sector_and_verifies_with_fn4() {
    let (mut s, h) = session();
    // Sector 3 is disabled in the fixture directory: refused, no fn3 frame.
    assert_eq!(
        select_profile(&mut s, 3).unwrap_err(),
        SectorWriteError::ProfileNotEnabled(3)
    );
    assert!(select_frames(&h).is_empty());
    // Out-of-range sectors are refused before any frame.
    let before = h.borrow().writes.len();
    for n in [0u16, 6, 0x0101, 0xFFFF] {
        assert!(matches!(
            select_profile(&mut s, n),
            Err(SectorWriteError::Patch(
                hidpp::onboard_write::WriteError::SelectSectorNotAllowed(_)
            ))
        ));
    }
    assert_eq!(h.borrow().writes.len(), before);

    // Enabled sectors can be selected; fn3 carries the sector BE16.
    let reply = select_profile(&mut s, 1).unwrap();
    assert_eq!(reply, "0".repeat(32));
    let frames = select_frames(&h);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0][3] >> 4, 3);
    assert_eq!(frames[0][4..6], [0x00, 0x01]);
    assert!(frames[0][6..].iter().all(|&b| b == 0));
    assert_eq!(h.borrow().active, 1);

    // After enabling sector 3 it can be selected too.
    write_directory(&mut s, 3, true, &mut nop_backup(), false).unwrap();
    select_profile(&mut s, 3).unwrap();
    assert_eq!(h.borrow().active, 3);
}

#[test]
fn select_profile_reports_a_failed_fn4_verification() {
    let (mut s, h) = session();
    h.borrow_mut().ignore_select = true;
    assert_eq!(
        select_profile(&mut s, 1).unwrap_err(),
        SectorWriteError::SelectVerifyFailed {
            expected: 1,
            got: 2
        }
    );
}

#[test]
fn restore_handles_the_directory_and_keeps_images_in_their_sectors() {
    let (mut s, h) = session();
    let dir = sector(&h, 0);
    let profile = sector(&h, 3);

    // A valid directory image goes to 0x0000 through the normal sequence.
    restore_sector(&mut s, 0, &dir).unwrap();
    assert_eq!(write_frames(&h).len(), 18);
    assert_eq!(sector(&h, 0), dir);

    // A profile image is not a directory, and a directory is not a profile.
    assert!(matches!(
        restore_sector(&mut s, 0, &profile),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    assert!(matches!(
        restore_sector(&mut s, 3, &dir),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    let mut bad_crc = dir.clone();
    bad_crc[0x20] ^= 1;
    assert!(matches!(
        restore_sector(&mut s, 0, &bad_crc),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    assert!(matches!(
        restore_sector(&mut s, 0, &dir[..254]),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    assert_eq!(write_frames(&h).len(), 18, "a refused image was written");
}

#[test]
fn directory_sector_is_still_not_reachable_through_the_read_or_volatile_paths() {
    let (mut s, h) = session();
    let before = h.borrow().writes.len();
    let mut begin = [0u8; 16];
    begin[5] = 255;
    assert!(
        s.request_feature(FeatureId::ONBOARD_PROFILES, 6, begin)
            .is_err()
    );
    assert!(
        s.request_volatile(FeatureId::ONBOARD_PROFILES, 6, begin)
            .is_err()
    );
    // fn3 is not reachable through them either.
    let mut select = [0u8; 16];
    select[1] = 1;
    assert!(
        s.request_feature(FeatureId::ONBOARD_PROFILES, 3, select)
            .is_err()
    );
    assert!(
        s.request_volatile(FeatureId::ONBOARD_PROFILES, 3, select)
            .is_err()
    );
    assert_eq!(h.borrow().writes.len(), before, "a frame was sent");
}

// ---- review round: enabling needs a valid profile, restore of the directory ----

#[test]
fn enabling_a_profile_needs_a_valid_target_profile() {
    let (mut s, h) = session();
    let original = sector(&h, 0);
    let mut backup = |_: u16, _: &[u8; 255]| -> Result<(), String> {
        panic!("the backup must not run when the target is invalid")
    };
    // Target CRC broken.
    h.borrow_mut().flash.get_mut(&3).unwrap()[0x40] ^= 1;
    assert!(matches!(
        write_directory(&mut s, 3, true, &mut backup, false).unwrap_err(),
        SectorWriteError::TargetProfileInvalid(_)
    ));
    // Target is not a profile at all (a directory image in sector 4).
    let dir = sector(&h, 0);
    h.borrow_mut().flash.insert(4, dir);
    assert!(matches!(
        write_directory(&mut s, 4, true, &mut backup, false).unwrap_err(),
        SectorWriteError::TargetProfileInvalid(_)
    ));
    assert!(write_frames(&h).is_empty());
    assert_eq!(sector(&h, 0), original);
    // Disabling does not look at the target.
    let report = write_directory(&mut s, 3, false, &mut nop_backup(), false).unwrap();
    assert_eq!(report.outcome, WriteOutcome::Unchanged);
}

#[test]
fn restore_of_the_directory_must_fit_the_current_one() {
    use hidpp::onboard_write::patch_directory;
    let (mut s, h) = session();
    let now: [u8; 255] = sector(&h, 0).try_into().unwrap();
    let with_crc = |mut d: [u8; 255]| {
        let c = crc::crc16_ccitt_false(&d[..253]);
        d[253..].copy_from_slice(&c.to_be_bytes());
        d.to_vec()
    };
    // Flags only, active (2) still enabled: accepted.
    let flags = *patch_directory(&now, 3, true, 2).unwrap().new.bytes();
    restore_sector(&mut s, 0, &flags).unwrap();
    assert_eq!(sector(&h, 0), flags.to_vec());
    assert_eq!(write_frames(&h).len(), 18);

    // A different entry list is refused, nothing written.
    let mut fewer = flags;
    fewer[16..20].copy_from_slice(&[0xFF; 4]);
    assert!(matches!(
        restore_sector(&mut s, 0, &with_crc(fewer)),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    // The active profile may not be disabled by the restore.
    h.borrow_mut().active = 3;
    assert!(matches!(
        restore_sector(&mut s, 0, &now),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    // Duplicate sector entries are malformed.
    let mut dup = flags;
    dup[16..18].copy_from_slice(&3u16.to_be_bytes());
    assert!(matches!(
        restore_sector(&mut s, 0, &with_crc(dup)),
        Err(SectorWriteError::ImageInvalid(_))
    ));
    assert_eq!(write_frames(&h).len(), 18, "a refused image was written");
}

// ---- save_active_profile ----

use logitech_g502x::{SaveError, save_active_profile};

/// Sector 3 enabled in the directory and active.
fn session_with_active(sector_nr: u16) -> (HidppSession<SimLink>, Handle) {
    let (mut s, h) = session();
    write_directory(&mut s, sector_nr, true, &mut nop_backup(), false).unwrap();
    h.borrow_mut().active = sector_nr;
    h.borrow_mut().writes.clear();
    h.borrow_mut().calls.clear();
    (s, h)
}

#[test]
fn save_active_profile_writes_and_reselects_the_same_sector() {
    let (mut s, h) = session_with_active(3);
    let original = sector(&h, 3);
    let saved: Saved = Default::default();
    let mut backup = recorder(&h, &saved);

    let report = save_active_profile(&mut s, &rate_patch(), &mut backup).unwrap();
    assert_eq!(report.sector, 3);
    assert_eq!(report.write.outcome, WriteOutcome::Written);
    assert_eq!(report.reselected.as_deref(), Some("0".repeat(32).as_str()));
    // The backup is of the active sector, before any write frame.
    assert_eq!(saved.borrow().as_slice(), &[(3, original.clone(), 0)]);
    assert_ne!(sector(&h, 3), original);
    assert_eq!(write_frames(&h).len(), 18);
    // fn3 for sector 3 comes after all write frames, and the sector stays active.
    let frames = select_frames(&h);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0][4..6], [0x00, 0x01]);
    assert_eq!(frames[1][4..6], [0x00, 0x03]);
    let writes = h.borrow().writes.clone();
    let last_write = writes
        .iter()
        .rposition(|f| f[2] == IDX_ONBOARD && f[3] >> 4 == 8)
        .unwrap();
    let select_at = writes
        .iter()
        .position(|f| f[2] == IDX_ONBOARD && f[3] >> 4 == 3)
        .unwrap();
    assert!(select_at > last_write);
    assert_eq!(h.borrow().active, 3);
}

#[test]
fn save_active_profile_unchanged_sends_no_write_and_no_reselect() {
    let (mut s, h) = session_with_active(3);
    let report = save_active_profile(&mut s, &ProfilePatch::default(), &mut nop_backup()).unwrap();
    assert_eq!(report.write.outcome, WriteOutcome::Unchanged);
    assert_eq!(report.reselected, None);
    assert!(write_frames(&h).is_empty());
    assert!(select_frames(&h).is_empty());
}

#[test]
fn save_active_profile_refuses_unwritable_active_profiles() {
    // Sector 2 (the user's own profile) is active in the fixture.
    let (mut s, h) = session();
    let before = h.borrow().writes.len();
    let mut backup =
        |_: u16, _: &[u8; 255]| -> Result<(), String> { panic!("no backup for a refused save") };
    let err = save_active_profile(&mut s, &rate_patch(), &mut backup).unwrap_err();
    assert!(
        matches!(err, SaveError::ProfileNotWritable { active: 2, .. }),
        "{err:?}"
    );
    // Host mode reports active sector 0.
    h.borrow_mut().active = 0;
    assert!(matches!(
        save_active_profile(&mut s, &rate_patch(), &mut backup).unwrap_err(),
        SaveError::ProfileNotWritable { active: 0, .. }
    ));
    // Sector 3 active but disabled in the directory.
    h.borrow_mut().active = 3;
    assert!(matches!(
        save_active_profile(&mut s, &rate_patch(), &mut backup).unwrap_err(),
        SaveError::ProfileNotWritable { active: 3, .. }
    ));
    assert!(write_frames(&h).is_empty());
    assert!(select_frames(&h).is_empty());
    let _ = before;
}

#[test]
fn save_active_profile_refuses_host_mode_before_anything_else() {
    let (mut s, h) = session_with_active(3);
    h.borrow_mut().mode = 2;
    let err = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap_err();
    assert_eq!(err, SaveError::NotOnboardMode);
    // Only the fn2 read went out.
    assert_eq!(h.borrow().writes.len(), 1);
    // An unknown mode byte is refused too.
    h.borrow_mut().mode = 7;
    assert_eq!(
        save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap_err(),
        SaveError::NotOnboardMode
    );
}

#[test]
fn save_active_profile_passes_write_failures_through() {
    let (mut s, h) = session_with_active(3);
    h.borrow_mut().faults.push((6, 1, Fault::DeviceError));
    let err = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap_err();
    assert!(matches!(
        err,
        SaveError::Write(SectorWriteError::WriteFailed { .. })
    ));
    assert!(select_frames(&h).is_empty());
}

// ---- reload by switching away and back ----

fn fn3_sectors(h: &Handle) -> Vec<u16> {
    select_frames(h)
        .iter()
        .map(|f| u16::from_be_bytes([f[4], f[5]]))
        .collect()
}

#[test]
fn save_reloads_by_switching_to_another_profile_and_back() {
    let (mut s, h) = session_with_active(3);
    let report = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap();
    assert!(report.reloaded);
    assert_eq!(report.reload_log.len(), 2);
    assert_eq!(
        report.reselected.as_deref(),
        Some(report.reload_log[1].as_str())
    );
    // Away to 0x0001 (preferred), back to 0x0003, both after the last write
    // frame, each verified with fn4 (the fn4 read of the save itself plus two).
    assert_eq!(fn3_sectors(&h), vec![1, 3]);
    assert_eq!(h.borrow().calls[&4], 3);
    assert_eq!(h.borrow().active, 3);
    let writes = h.borrow().writes.clone();
    let last_write = writes
        .iter()
        .rposition(|f| f[2] == IDX_ONBOARD && f[3] >> 4 == 8)
        .unwrap();
    let first_select = writes
        .iter()
        .position(|f| f[2] == IDX_ONBOARD && f[3] >> 4 == 3)
        .unwrap();
    assert!(first_select > last_write);
    // fn4 follows each fn3.
    let seq: Vec<u8> = writes[last_write..]
        .iter()
        .filter(|f| f[2] == IDX_ONBOARD && matches!(f[3] >> 4, 3 | 4))
        .map(|f| f[3] >> 4)
        .collect();
    assert_eq!(seq, vec![3, 4, 3, 4]);
}

#[test]
fn save_prefers_sector_2_when_sector_1_is_disabled_and_falls_back_to_any_enabled() {
    let (mut s, h) = session_with_active(3);
    // Disable 0x0001 in the directory image directly (the active one stays).
    {
        let mut sim = h.borrow_mut();
        let dir = sim.flash.get_mut(&0).unwrap();
        dir[2] = 0;
        let c = crc::crc16_ccitt_false(&dir[..253]);
        dir[253..].copy_from_slice(&c.to_be_bytes());
    }
    save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap();
    assert_eq!(fn3_sectors(&h), vec![2, 3]);
}

#[test]
fn save_without_another_enabled_profile_falls_back_to_a_single_reselect() {
    let (mut s, h) = session_with_active(3);
    {
        let mut sim = h.borrow_mut();
        let dir = sim.flash.get_mut(&0).unwrap();
        dir[2] = 0; // sector 1
        dir[6] = 0; // sector 2
        let c = crc::crc16_ccitt_false(&dir[..253]);
        dir[253..].copy_from_slice(&c.to_be_bytes());
    }
    let report = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap();
    assert!(!report.reloaded);
    assert_eq!(report.write.outcome, WriteOutcome::Written);
    assert_eq!(report.reload_log.len(), 1);
    assert!(report.reselected.is_some());
    assert_eq!(fn3_sectors(&h), vec![3]);
}

#[test]
fn save_retries_the_switch_back_once() {
    let (mut s, h) = session_with_active(3);
    // fn3 call 1 goes away, call 2 (the way back) fails, call 3 is the retry.
    h.borrow_mut().faults.push((3, 2, Fault::DeviceError));
    let report = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap();
    assert!(report.reloaded);
    assert_eq!(fn3_sectors(&h), vec![1, 3, 3]);
    assert_eq!(report.reload_log.len(), 2);
    assert_eq!(h.borrow().active, 3);
}

#[test]
fn save_reports_the_active_sector_when_the_switch_back_fails_twice() {
    let (mut s, h) = session_with_active(3);
    {
        let mut sim = h.borrow_mut();
        sim.faults.push((3, 2, Fault::DeviceError));
        sim.faults.push((3, 3, Fault::DeviceError));
    }
    let err = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap_err();
    let SaveError::Reselect {
        report,
        cause,
        active_now,
    } = err
    else {
        panic!("{err:?}");
    };
    assert_eq!(report.outcome, WriteOutcome::Written);
    assert!(matches!(
        cause,
        SectorWriteError::Session(SessionError::Device { code: 2, .. })
    ));
    // The mouse is still on the other profile; fn4 says so.
    assert_eq!(active_now, Some(1));
    assert_eq!(h.borrow().active, 1);
    assert_eq!(fn3_sectors(&h), vec![1, 3, 3]);
}

#[test]
fn a_failed_switch_away_still_switches_back_and_is_not_a_reload() {
    let (mut s, h) = session_with_active(3);
    // fn3 call 1 (away) is refused by the mouse; call 2 (back) works.
    h.borrow_mut().faults.push((3, 1, Fault::DeviceError));
    let report = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap();
    assert_eq!(report.write.outcome, WriteOutcome::Written);
    assert!(!report.reloaded);
    assert!(report.reselected.is_some());
    assert_eq!(fn3_sectors(&h), vec![1, 3]);
    assert_eq!(report.reload_log.len(), 2);
    assert!(
        report.reload_log[0].contains("switch away"),
        "{:?}",
        report.reload_log
    );
    assert_eq!(h.borrow().active, 3);
}

#[test]
fn a_switch_away_that_fails_its_fn4_verify_still_switches_back() {
    let (mut s, h) = session_with_active(3);
    // fn3 is acknowledged but the mouse stays on 3: the away verify fails.
    h.borrow_mut().ignore_select = true;
    let report = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap();
    assert!(!report.reloaded);
    assert_eq!(fn3_sectors(&h), vec![1, 3]);
    assert!(report.reload_log[0].contains("switch away"));
    assert_eq!(h.borrow().active, 3);
}

#[test]
fn a_switch_away_that_landed_but_failed_its_verify_is_undone_by_the_switch_back() {
    let (mut s, h) = session_with_active(3);
    // The away fn3 lands (active becomes 1) but its fn4 verify gets no reply.
    // The first fn4 call of the save is its own read; the verify is call 2.
    h.borrow_mut().faults.push((4, 2, Fault::Silent));
    let report = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap();
    assert!(!report.reloaded);
    assert_eq!(fn3_sectors(&h), vec![1, 3]);
    assert_eq!(h.borrow().active, 3);
}

#[test]
fn reselect_error_only_when_the_mouse_is_not_on_the_saved_profile() {
    // Away fails, back fails twice, but the mouse never left the target: Ok.
    let (mut s, h) = session_with_active(3);
    {
        let mut sim = h.borrow_mut();
        sim.faults.push((3, 1, Fault::DeviceError));
        sim.faults.push((3, 2, Fault::DeviceError));
        sim.faults.push((3, 3, Fault::DeviceError));
    }
    let report = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap();
    assert!(!report.reloaded);
    assert_eq!(report.reselected, None);
    assert_eq!(fn3_sectors(&h), vec![1, 3, 3]);
    assert!(
        report
            .reload_log
            .iter()
            .any(|l| l.contains("fn4 shows it active"))
    );
    // Away works, back fails twice: the mouse is on 1, so it is an error.
    let (mut s, h) = session_with_active(3);
    {
        let mut sim = h.borrow_mut();
        sim.faults.push((3, 2, Fault::DeviceError));
        sim.faults.push((3, 3, Fault::DeviceError));
    }
    let err = save_active_profile(&mut s, &rate_patch(), &mut nop_backup()).unwrap_err();
    assert!(matches!(
        err,
        SaveError::Reselect {
            active_now: Some(1),
            ..
        }
    ));
}
