//! Single-key keymap write (VIA 05) over the mock link. Nothing here opens a device.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::{Handle, Mock, pad};
use keychron_k2he::{Session, SessionError, set_keycode};
use keychron_proto::PolicyError;

const COLS: usize = 16;
const ROWS: usize = 6;

struct Kb {
    /// 4 layers x 6 rows x 16 columns.
    keys: Vec<u16>,
    /// VIA 05 changes the key.
    apply: bool,
    /// VIA 05 echoes this keycode instead of the sent one.
    echo: Option<u16>,
    answer05: bool,
}

fn keyboard(kb: Kb) -> (Session<Mock>, Handle, Rc<RefCell<Kb>>) {
    let state = Rc::new(RefCell::new(kb));
    let st = Rc::clone(&state);
    let mock = Mock::new(move |_, f| {
        let mut k = st.borrow_mut();
        match f[0] {
            0x11 => vec![pad(&[0x11, 4]).to_vec()],
            0x12 => {
                let off = usize::from(f[1]) << 8 | usize::from(f[2]);
                assert_eq!(f[3], 2, "only one key is read");
                let mut r = pad(&f[..4]);
                r[4..6].copy_from_slice(&k.keys[off / 2].to_be_bytes());
                vec![r.to_vec()]
            }
            0x05 => {
                let index =
                    (usize::from(f[1]) * ROWS + usize::from(f[2])) * COLS + usize::from(f[3]);
                let code = u16::from_be_bytes([f[4], f[5]]);
                if k.apply {
                    k.keys[index] = code;
                }
                let mut r = *f;
                if let Some(e) = k.echo {
                    r[4..6].copy_from_slice(&e.to_be_bytes());
                }
                if k.answer05 { vec![r.to_vec()] } else { vec![] }
            }
            _ => panic!("unexpected request {:02x?}", &f[..6]),
        }
    });
    let handle = mock.handle();
    let mut s = Session::with_link(mock);
    s.set_reply_timeout(Duration::from_millis(200));
    (s, handle, state)
}

fn kb() -> Kb {
    let mut keys = vec![0x0004u16; 4 * ROWS * COLS];
    // Win base layer 2, r0c15 RGB_MOD and r0c14 KC_DEL (the planned remap).
    keys[(2 * ROWS) * COLS + 15] = 0x7821;
    keys[(2 * ROWS) * COLS + 14] = 0x004C;
    Kb {
        keys,
        apply: true,
        echo: None,
        answer05: true,
    }
}

fn rep(bytes: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8];
    r.extend_from_slice(&pad(bytes));
    r
}

fn sent(h: &Handle) -> Vec<Vec<u8>> {
    h.borrow().written.iter().map(|r| r.to_vec()).collect()
}

fn count(h: &Handle, first: u8) -> usize {
    sent(h).iter().filter(|r| r[1] == first).count()
}

#[test]
fn writes_one_frame_and_reads_the_key_back() {
    let (mut s, h, state) = keyboard(kb());
    let out = set_keycode(&mut s, 2, 0, 15, 0x004C).unwrap();
    // Offset ((2*6+0)*16+15)*2 = 414 = 0x019E.
    assert_eq!(
        sent(&h),
        [
            rep(&[0x11]),
            rep(&[0x12, 0x01, 0x9E, 2]),
            rep(&[0x05, 2, 0, 15, 0x00, 0x4C]),
            rep(&[0x12, 0x01, 0x9E, 2]),
        ]
    );
    assert_eq!((out.before, out.after), (0x7821, 0x004C));
    assert_eq!((out.layer, out.row, out.col), (2, 0, 15));
    assert_eq!(out.frames_written, 1);
    assert!(!out.unchanged);
    assert_eq!(state.borrow().keys[2 * ROWS * COLS + 15], 0x004C);
}

#[test]
fn a_key_that_already_holds_the_code_writes_nothing() {
    let (mut s, h, _) = keyboard(kb());
    let out = set_keycode(&mut s, 2, 0, 14, 0x004C).unwrap();
    assert!(out.unchanged);
    assert_eq!(out.frames_written, 0);
    assert_eq!(out.before, out.after);
    assert_eq!(count(&h, 0x05), 0);
}

#[test]
fn a_value_that_did_not_stick_is_a_verify_mismatch() {
    let mut k = kb();
    k.apply = false;
    let (mut s, h, _) = keyboard(k);
    let err = set_keycode(&mut s, 2, 0, 15, 0x004C).unwrap_err();
    assert!(matches!(
        err,
        SessionError::VerifyMismatch {
            what: "keycode",
            saved: true,
            ..
        }
    ));
    assert_eq!(count(&h, 0x05), 1, "the write is never resent");
}

#[test]
fn a_different_keycode_in_the_echo_is_a_mismatch() {
    let mut k = kb();
    k.echo = Some(0x0004);
    let (mut s, h, _) = keyboard(k);
    assert!(matches!(
        set_keycode(&mut s, 2, 0, 15, 0x004C).unwrap_err(),
        SessionError::VerifyMismatch { .. }
    ));
    assert_eq!(count(&h, 0x12), 2, "the key is still read back");
}

#[test]
fn a_write_timeout_is_not_retried() {
    let mut k = kb();
    k.answer05 = false;
    let (mut s, h, _) = keyboard(k);
    assert_eq!(
        set_keycode(&mut s, 2, 0, 15, 0x004C).unwrap_err(),
        SessionError::Timeout
    );
    assert_eq!(count(&h, 0x05), 1);
}

#[test]
fn out_of_range_keys_and_forbidden_codes_are_refused_before_any_write_or_read() {
    for (layer, row, col, code) in [
        (4, 0, 0, 0x004C),  // layer past the count read from the keyboard
        (0, 6, 0, 0x004C),  // row
        (0, 0, 16, 0x004C), // column
        (0, 0, 0, 0x7C00),  // QK_BOOT
        (0, 0, 0, 0x7C03),  // QK_CLEAR_EEPROM
        (0, 0, 0, 0x5223),  // not in the allowlist
        (0, 0, 0, 0xBEEF),  // unknown
    ] {
        let (mut s, h, _) = keyboard(kb());
        let err = set_keycode(&mut s, layer, row, col, code).unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::Policy(PolicyError::OutOfRange { .. } | PolicyError::Invalid { .. })
            ),
            "{layer} {row} {col} {code:#06x}: {err:?}"
        );
        assert_eq!(count(&h, 0x05), 0);
        assert_eq!(count(&h, 0x12), 0, "refused before the key is read");
    }
}

#[test]
fn the_session_policy_stays_read_only() {
    let (mut s, _h, _) = keyboard(kb());
    set_keycode(&mut s, 2, 0, 15, 0x004C).unwrap();
    assert_eq!(
        s.request(keychron_proto::Request::SetKeycode {
            layer: 0,
            row: 0,
            col: 0,
            code: 0x0004
        }),
        Err(SessionError::Policy(PolicyError::TierNotEnabled {
            tier: keychron_proto::Tier::Persistent
        }))
    );
}

#[test]
fn a_key_holding_a_code_myhub_cannot_restore_is_not_overwritten() {
    let mut k = kb();
    k.keys[2 * ROWS * COLS + 15] = 0x5221; // MO(1)
    let (mut s, h, state) = keyboard(k);
    let err = set_keycode(&mut s, 2, 0, 15, 0x004C).unwrap_err();
    assert!(matches!(
        err,
        SessionError::Policy(PolicyError::Invalid {
            field: "current keycode",
            ..
        })
    ));
    assert_eq!(count(&h, 0x05), 0);
    assert_eq!(state.borrow().keys[2 * ROWS * COLS + 15], 0x5221);
}

#[test]
fn a_wrong_echo_still_reads_the_key_and_reports_its_real_value() {
    let mut k = kb();
    k.echo = Some(0x0004);
    k.apply = false;
    let (mut s, h, _) = keyboard(k);
    match set_keycode(&mut s, 2, 0, 15, 0x004C).unwrap_err() {
        SessionError::VerifyMismatch { got, .. } => assert_eq!(got, "0x7821"),
        e => panic!("{e:?}"),
    }
    assert_eq!(count(&h, 0x05), 1);
    assert_eq!(count(&h, 0x12), 2);
}
