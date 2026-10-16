//! Travel (A9 14) and lighting (VIA 07) writes over the mock link. Nothing here opens a
//! device.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::{Handle, Mock, hex, pad};
use keychron_k2he::{
    LightChange, LightState, Session, SessionError, TravelSettings, set_lighting, set_travel_global,
};
use keychron_proto::he::KeyMode;
use keychron_proto::{LightValue, PolicyError, Request, Tier, TravelMode};

/// Global word: mode, distance, press, release (index and advance mode zero).
fn word(mode: u32, distance: u32, press: u32, release: u32) -> u32 {
    mode | distance << 2 | press << 8 | release << 14
}

struct Kb {
    current: u8,
    word: u32,
    /// A9 14 changes the word.
    apply: bool,
    status14: u8,
    status1f: u8,
    answer14: bool,
    /// A9 10 reports this profile after the first A9 12 read of the word (hotkey).
    hotkey_to: Option<u8>,
    /// [brightness, effect, speed, hue, saturation]
    light: [u8; 5],
    apply_light: bool,
    /// Brightness is stored this much lower than sent [OBS: 1].
    bright_delta: u8,
}

impl Kb {
    fn new() -> Self {
        Kb {
            current: 0,
            word: word(1, 20, 4, 4),
            apply: true,
            status14: 0,
            status1f: 0,
            answer14: true,
            hotkey_to: None,
            light: [100, 6, 50, 10, 20],
            apply_light: true,
            bright_delta: 0,
        }
    }
}

fn keyboard(kb: Kb) -> (Session<Mock>, Handle, Rc<RefCell<Kb>>) {
    let state = Rc::new(RefCell::new(kb));
    let st = Rc::clone(&state);
    let mock = Mock::new(move |_, f| {
        let mut k = st.borrow_mut();
        match (f[0], f[1]) {
            (0xA9, 0x10) => {
                let mut r = pad(&hex("a9 10 00 03 5c 03 14 14"));
                r[2] = k.current;
                vec![r.to_vec()]
            }
            (0xA9, 0x12) => {
                assert_eq!(
                    (f[3], f[4], f[5]),
                    (0, 0, 4),
                    "only the global word is read"
                );
                let mut r = pad(&f[..6]);
                r[6..10].copy_from_slice(&k.word.to_le_bytes());
                if let Some(p) = k.hotkey_to.take() {
                    k.current = p;
                }
                vec![r.to_vec()]
            }
            (0xA9, 0x14) => {
                if k.apply {
                    let mode = u32::from(f[3]);
                    k.word = word(mode, f[4].into(), f[5].into(), f[6].into());
                }
                if k.answer14 {
                    vec![pad(&[0xA9, 0x14, k.status14]).to_vec()]
                } else {
                    vec![]
                }
            }
            (0xA9, 0x1F) => vec![pad(&[0xA9, 0x1F, k.status1f]).to_vec()],
            (0x08, 0x03) => {
                let i = usize::from(f[2]);
                let mut r = pad(&f[..3]);
                match i {
                    // ids: 1 brightness, 2 effect, 3 speed
                    1..=3 => r[3] = k.light[i - 1],
                    4 => r[3..5].copy_from_slice(&k.light[3..5]),
                    _ => panic!("light id {i}"),
                }
                vec![r.to_vec()]
            }
            (0x07, 0x03) => {
                if k.apply_light {
                    match f[2] {
                        1 => k.light[0] = f[3].saturating_sub(k.bright_delta),
                        2 => k.light[1] = f[3],
                        3 => k.light[2] = f[3],
                        4 => k.light[3..5].copy_from_slice(&f[3..5]),
                        _ => panic!("light id"),
                    }
                }
                vec![f.to_vec()]
            }
            (0x09, 0x03) => vec![f.to_vec()],
            _ => panic!("unexpected request {:02x?}", &f[..6]),
        }
    });
    let handle = mock.handle();
    let mut s = Session::with_link(mock);
    s.set_reply_timeout(Duration::from_millis(200));
    (s, handle, state)
}

fn rep(bytes: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8];
    r.extend_from_slice(&pad(bytes));
    r
}

fn sent(h: &Handle) -> Vec<Vec<u8>> {
    h.borrow().written.iter().map(|r| r.to_vec()).collect()
}

fn count(h: &Handle, prefix: &[u8]) -> usize {
    sent(h)
        .iter()
        .filter(|r| r[1..].starts_with(prefix))
        .count()
}

const READ_WORD: [u8; 6] = [0xA9, 0x12, 0, 0, 0, 4];

fn rt(actuation: u8, press: u8, release: u8) -> TravelSettings {
    TravelSettings {
        mode: TravelMode::RapidTrigger,
        actuation,
        press,
        release,
    }
}

#[test]
fn travel_rapid_trigger_sends_exact_frames_without_save() {
    let (mut s, h, _) = keyboard(Kb::new());
    let out = set_travel_global(&mut s, rt(18, 3, 5), false).unwrap();
    assert_eq!(
        sent(&h),
        [
            rep(&[0xA9, 0x10]),
            rep(&READ_WORD),
            rep(&[0xA9, 0x14, 0, 2, 18, 3, 5, 1]),
            rep(&READ_WORD),
        ]
    );
    assert_eq!(count(&h, &[0xA9, 0x1F]), 0);
    assert_eq!(out.profile, 0);
    assert_eq!(out.before.mode, KeyMode::Static);
    assert_eq!(
        (
            out.after.mode,
            out.after.distance,
            out.after.press,
            out.after.release
        ),
        (KeyMode::RapidTrigger, 18, 3, 5)
    );
    assert!(!out.saved && !out.unchanged);
    assert_eq!(out.frames_written, 1);
}

#[test]
fn travel_static_keeps_the_press_and_release_of_before() {
    let mut kb = Kb::new();
    kb.current = 1;
    kb.word = word(2, 20, 3, 5);
    let (mut s, h, _) = keyboard(kb);
    let settings = TravelSettings {
        mode: TravelMode::Static,
        actuation: 30,
        press: 9,
        release: 9,
    };
    let out = set_travel_global(&mut s, settings, false).unwrap();
    assert_eq!(
        sent(&h)[2],
        rep(&[0xA9, 0x14, 1, 1, 30, 3, 5, 1]),
        "profile 1, static, press/release from before"
    );
    assert_eq!(
        (out.after.mode, out.after.distance, out.after.press),
        (KeyMode::Static, 30, 3)
    );
}

#[test]
fn travel_unchanged_without_save_writes_nothing() {
    let (mut s, h, _) = keyboard(Kb::new());
    // Static 2.0 mm already; press and release in the request are ignored for Static.
    let settings = TravelSettings {
        mode: TravelMode::Static,
        actuation: 20,
        press: 9,
        release: 9,
    };
    let out = set_travel_global(&mut s, settings, false).unwrap();
    assert_eq!(sent(&h), [rep(&[0xA9, 0x10]), rep(&READ_WORD)]);
    assert!(out.unchanged && !out.saved);
    assert_eq!(out.frames_written, 0);
    assert_eq!(out.before, out.after);
}

#[test]
fn travel_save_reads_the_profile_again_then_sends_a9_1f() {
    let mut kb = Kb::new();
    kb.current = 2;
    let (mut s, h, _) = keyboard(kb);
    let out = set_travel_global(&mut s, rt(18, 3, 5), true).unwrap();
    assert_eq!(
        sent(&h),
        [
            rep(&[0xA9, 0x10]),
            rep(&[0xA9, 0x12, 2, 0, 0, 4]),
            rep(&[0xA9, 0x14, 2, 2, 18, 3, 5, 1]),
            rep(&[0xA9, 0x12, 2, 0, 0, 4]),
            rep(&[0xA9, 0x10]),
            rep(&[0xA9, 0x1F, 2]),
            rep(&[0xA9, 0x12, 2, 0, 0, 4]),
        ]
    );
    assert!(out.saved && !out.unchanged);
    assert_eq!(out.frames_written, 2);
}

#[test]
fn travel_unchanged_with_save_sends_only_a9_1f() {
    let (mut s, h, _) = keyboard(Kb::new());
    let settings = TravelSettings {
        mode: TravelMode::Static,
        actuation: 20,
        press: 1,
        release: 1,
    };
    let out = set_travel_global(&mut s, settings, true).unwrap();
    assert_eq!(count(&h, &[0xA9, 0x14]), 0);
    assert_eq!(count(&h, &[0xA9, 0x1F]), 1);
    assert!(out.saved && !out.unchanged);
    assert_eq!(out.frames_written, 1);
}

#[test]
fn travel_nonzero_status_is_a_device_status_error() {
    let mut kb = Kb::new();
    kb.status14 = 2;
    let (mut s, h, _) = keyboard(kb);
    assert_eq!(
        set_travel_global(&mut s, rt(18, 3, 5), true).unwrap_err(),
        SessionError::DeviceStatus {
            command: 0xA9,
            sub: 0x14,
            status: 2
        }
    );
    assert_eq!(count(&h, &[0xA9, 0x1F]), 0);

    let mut kb = Kb::new();
    kb.status1f = 3;
    let (mut s, _h, _) = keyboard(kb);
    assert_eq!(
        set_travel_global(&mut s, rt(18, 3, 5), true).unwrap_err(),
        SessionError::DeviceStatus {
            command: 0xA9,
            sub: 0x1F,
            status: 3
        }
    );
}

#[test]
fn travel_a_write_timeout_is_not_retried() {
    let mut kb = Kb::new();
    kb.answer14 = false;
    let (mut s, h, _) = keyboard(kb);
    assert_eq!(
        set_travel_global(&mut s, rt(18, 3, 5), false).unwrap_err(),
        SessionError::Timeout
    );
    assert_eq!(count(&h, &[0xA9, 0x14]), 1);
}

#[test]
fn travel_a_value_that_did_not_stick_fails_verification_without_a_save() {
    let mut kb = Kb::new();
    kb.apply = false;
    let (mut s, h, _) = keyboard(kb);
    let err = set_travel_global(&mut s, rt(18, 3, 5), true).unwrap_err();
    assert!(matches!(
        err,
        SessionError::VerifyMismatch {
            what: "travel",
            saved: false,
            ..
        }
    ));
    assert_eq!(count(&h, &[0xA9, 0x1F]), 0);
}

#[test]
fn travel_save_is_refused_when_the_profile_changed_on_the_keyboard() {
    let mut kb = Kb::new();
    kb.hotkey_to = Some(1);
    let (mut s, h, _) = keyboard(kb);
    assert_eq!(
        set_travel_global(&mut s, rt(18, 3, 5), true).unwrap_err(),
        SessionError::ProfileChanged {
            expected: 0,
            got: 1
        }
    );
    assert_eq!(count(&h, &[0xA9, 0x1F]), 0);
}

#[test]
fn travel_static_substitutes_04_mm_for_a_stored_press_release_outside_the_range() {
    let mut kb = Kb::new();
    kb.word = word(1, 20, 0, 0);
    let (mut s, h, _) = keyboard(kb);
    let settings = TravelSettings {
        mode: TravelMode::Static,
        actuation: 30,
        press: 9,
        release: 9,
    };
    let out = set_travel_global(&mut s, settings, false).unwrap();
    assert_eq!(sent(&h)[2], rep(&[0xA9, 0x14, 0, 1, 30, 4, 4, 1]));
    assert_eq!((out.after.press, out.after.release), (4, 4));
}

#[test]
fn travel_out_of_range_values_are_refused_before_the_frame() {
    let (mut s, h, _) = keyboard(Kb::new());
    let err = set_travel_global(&mut s, rt(39, 3, 5), false).unwrap_err();
    assert!(matches!(
        err,
        SessionError::Policy(PolicyError::OutOfRange { .. })
    ));
    assert_eq!(count(&h, &[0xA9, 0x14]), 0);
}

const READ_BRIGHTNESS: [u8; 3] = [0x08, 0x03, 1];
const READ_EFFECT: [u8; 3] = [0x08, 0x03, 2];
const READ_SPEED: [u8; 3] = [0x08, 0x03, 3];
const READ_COLOR: [u8; 3] = [0x08, 0x03, 4];

fn reads_before() -> Vec<Vec<u8>> {
    [READ_BRIGHTNESS, READ_EFFECT, READ_SPEED, READ_COLOR]
        .iter()
        .map(|r| rep(r))
        .collect()
}

#[test]
fn lighting_single_field_writes_one_frame_and_reads_it_back() {
    let (mut s, h, _) = keyboard(Kb::new());
    let out = set_lighting(
        &mut s,
        LightChange {
            brightness: Some(200),
            ..LightChange::default()
        },
        false,
    )
    .unwrap();
    let mut expected = reads_before();
    expected.push(rep(&[0x07, 0x03, 1, 200]));
    expected.extend(reads_before());
    assert_eq!(sent(&h), expected);
    assert_eq!((out.before.brightness, out.after.brightness), (100, 200));
    assert_eq!(out.before.effect, out.after.effect);
    assert!(!out.saved && !out.unchanged);
    assert_eq!(out.frames_written, 1);
    assert_eq!(count(&h, &[0x09, 0x03]), 0);
}

#[test]
fn lighting_several_fields_go_effect_brightness_speed_color_and_skip_unchanged() {
    let (mut s, h, _) = keyboard(Kb::new());
    let out = set_lighting(
        &mut s,
        LightChange {
            effect: Some(3),
            brightness: Some(100), // already 100: skipped
            speed: Some(7),
            color: Some((200, 255)),
        },
        false,
    )
    .unwrap();
    let mut expected = reads_before();
    expected.extend([
        rep(&[0x07, 0x03, 2, 3]),
        rep(&[0x07, 0x03, 3, 7]),
        rep(&[0x07, 0x03, 4, 200, 255]),
    ]);
    expected.extend(reads_before());
    assert_eq!(sent(&h), expected);
    assert_eq!(
        out.after,
        LightState {
            brightness: 100,
            effect: 3,
            speed: 7,
            hue: 200,
            saturation: 255
        }
    );
    assert_eq!(out.frames_written, 3);
}

#[test]
fn lighting_nothing_to_change_writes_nothing() {
    let (mut s, h, _) = keyboard(Kb::new());
    let out = set_lighting(
        &mut s,
        LightChange {
            effect: Some(6),
            ..LightChange::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(sent(&h), reads_before());
    assert!(out.unchanged && out.frames_written == 0);
}

#[test]
fn lighting_save_sends_09_03_then_reads_all_four_again() {
    let (mut s, h, _) = keyboard(Kb::new());
    let out = set_lighting(
        &mut s,
        LightChange {
            speed: Some(9),
            ..LightChange::default()
        },
        true,
    )
    .unwrap();
    let mut expected = reads_before();
    expected.push(rep(&[0x07, 0x03, 3, 9]));
    expected.extend(reads_before());
    expected.push(rep(&[0x09, 0x03]));
    expected.extend(reads_before());
    assert_eq!(sent(&h), expected);
    assert!(out.saved);
    assert_eq!(out.frames_written, 2);
}

#[test]
fn lighting_a_value_that_did_not_stick_fails_verification_without_a_save() {
    let mut kb = Kb::new();
    kb.apply_light = false;
    let (mut s, h, _) = keyboard(kb);
    let err = set_lighting(
        &mut s,
        LightChange {
            speed: Some(9),
            ..LightChange::default()
        },
        true,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        SessionError::VerifyMismatch {
            what: "lighting",
            saved: false,
            ..
        }
    ));
    assert_eq!(count(&h, &[0x09, 0x03]), 0);
}

#[test]
fn lighting_effect_out_of_range_is_refused_before_any_write() {
    let (mut s, h, _) = keyboard(Kb::new());
    let err = set_lighting(
        &mut s,
        LightChange {
            effect: Some(25),
            speed: Some(9),
            ..LightChange::default()
        },
        false,
    )
    .unwrap_err();
    assert!(matches!(err, SessionError::Policy(_)));
    assert_eq!(count(&h, &[0x07]), 0);
}

#[test]
fn the_session_policy_stays_read_only_after_both_writes() {
    let (mut s, _h, _) = keyboard(Kb::new());
    set_travel_global(&mut s, rt(18, 3, 5), true).unwrap();
    set_lighting(
        &mut s,
        LightChange {
            speed: Some(9),
            ..LightChange::default()
        },
        true,
    )
    .unwrap();
    for (request, tier) in [
        (
            Request::SetTravelGlobal {
                profile: 0,
                mode: TravelMode::Static,
                actuation: 20,
                press: 4,
                release: 4,
            },
            Tier::Volatile,
        ),
        (
            Request::SetLight {
                value: LightValue::Speed(1),
            },
            Tier::Volatile,
        ),
        (Request::SaveProfile { profile: 0 }, Tier::Persistent),
        (Request::SaveLighting, Tier::Persistent),
    ] {
        assert_eq!(
            s.request(request),
            Err(SessionError::Policy(PolicyError::TierNotEnabled { tier })),
            "{request:?}"
        );
    }
}

fn bright(v: u8) -> LightChange {
    LightChange {
        brightness: Some(v),
        ..LightChange::default()
    }
}

#[test]
fn lighting_brightness_stored_one_lower_is_accepted_and_reported_as_held() {
    let mut kb = Kb::new();
    kb.bright_delta = 1;
    let (mut s, _h, _) = keyboard(kb);
    let out = set_lighting(&mut s, bright(120), false).unwrap();
    assert_eq!(out.after.brightness, 119);
    assert_eq!(out.frames_written, 1);
}

#[test]
fn lighting_brightness_outside_the_tolerance_fails_verification() {
    let mut kb = Kb::new();
    kb.bright_delta = 5;
    let (mut s, h, _) = keyboard(kb);
    let err = set_lighting(&mut s, bright(120), true).unwrap_err();
    assert!(matches!(
        err,
        SessionError::VerifyMismatch {
            what: "lighting",
            saved: false,
            ..
        }
    ));
    assert_eq!(count(&h, &[0x09, 0x03]), 0);
}

#[test]
fn lighting_effect_must_match_exactly() {
    let mut kb = Kb::new();
    kb.apply_light = false;
    let (mut s, _h, _) = keyboard(kb);
    let err = set_lighting(
        &mut s,
        LightChange {
            effect: Some(7),
            ..LightChange::default()
        },
        false,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        SessionError::VerifyMismatch { saved: false, .. }
    ));
}

#[test]
fn lighting_a_value_within_tolerance_of_the_current_one_is_unchanged() {
    let (mut s, h, _) = keyboard(Kb::new());
    let out = set_lighting(&mut s, bright(102), false).unwrap();
    assert!(out.unchanged);
    assert_eq!(count(&h, &[0x07]), 0);
}

#[test]
fn lighting_save_verifies_exactly_against_the_read_back() {
    let mut kb = Kb::new();
    kb.bright_delta = 1;
    let (mut s, h, _) = keyboard(kb);
    let out = set_lighting(&mut s, bright(120), true).unwrap();
    assert!(out.saved);
    assert_eq!(out.after.brightness, 119);
    assert_eq!(count(&h, &[0x09, 0x03]), 1);
}
