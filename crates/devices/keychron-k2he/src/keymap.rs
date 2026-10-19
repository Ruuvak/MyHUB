//! Single-key keymap write: VIA 05 dynamic_keymap_set_keycode (Persistent).
//!
//! Same rules as `travel.rs` and `light.rs`: one-call policy, the write frame is never
//! resent, writes are at least `WRITE_GAP` apart, reads inside the sequence are sent once,
//! and the write is verified by reading the key back (VIA 12, 2 bytes). Any error after
//! the write frame went out (Timeout, AmbiguousReply, ForeignTraffic, BadReport, a link
//! error, VerifyMismatch) means the state of that key is unknown: read it again (`state`)
//! before deciding anything.
//!
//! [INF] VIA 05 saves to EEPROM at once, there is no separate save command, so a verified
//! write is a flash write; confirmed on hardware only after the first supervised run.
//! [INF] The reply echoes the six request bytes. Bytes 0..=3 are matched by the session;
//! bytes 4..=5 are compared here, but the read-back is the real check.
//!
//! The keycode must pass `keychron_proto::keycode::check_writable` (basic keys, KC_NO,
//! KC_TRNS, RGB, named Keychron keys; never the QMK 0x7C00 block with boot, reboot and
//! clear EEPROM). Layer, row and column are 0-based and bounded by the layer count read
//! from the keyboard (VIA 11) and the matrix; all of it is checked before the first frame
//! of the write sequence is built.

use keychron_proto::keycode::check_writable;
use keychron_proto::via::decode_layer_count;
use keychron_proto::{PolicyError, Request, Tier};
use serde::Serialize;

use crate::error::SessionError;
use crate::link::RawLink;
use crate::session::Session;
use crate::{MATRIX_COLS, MATRIX_ROWS};

/// What [`set_keycode`] did. Layer, row and column are 0-based, as on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KeycodeOutcome {
    pub layer: u8,
    pub row: u8,
    pub col: u8,
    /// Keycode of the key before the call.
    pub before: u16,
    /// Keycode read back at the end.
    pub after: u16,
    /// VIA 05 frames sent (0 or 1).
    pub frames_written: u32,
    /// Nothing was written: the key already held the code.
    pub unchanged: bool,
}

/// Byte offset of a key in the VIA keymap buffer: 2 bytes per key, layer-major, then row,
/// then column. Callers bound the indexes first.
fn offset(layer: u8, row: u8, col: u8) -> u16 {
    let key = (u32::from(layer) * u32::from(MATRIX_ROWS) + u32::from(row)) * u32::from(MATRIX_COLS)
        + u32::from(col);
    // Bounded by layers * rows * cols * 2 <= 255 * 6 * 16 * 2 = 48960, inside u16.
    u16::try_from(key * 2).unwrap_or(u16::MAX)
}

/// The keycode at (layer, row, col): VIA 12, 2 bytes, big-endian. Needs the matrix and
/// layer count limits, which [`set_keycode`] sets.
fn read_key<L: RawLink>(
    s: &mut Session<L>,
    layer: u8,
    row: u8,
    col: u8,
) -> Result<u16, SessionError> {
    let r = s.request(Request::GetKeymapBuffer {
        offset: offset(layer, row, col),
        len: 2,
    })?;
    Ok(u16::from_be_bytes([r[4], r[5]]))
}

fn mismatch(expected: u16, got: u16) -> SessionError {
    SessionError::VerifyMismatch {
        what: "keycode",
        expected: format!("{expected:#06x}"),
        got: format!("{got:#06x}"),
        // VIA 05 saves at once [INF].
        saved: true,
    }
}

/// Put `code` on one key of the base or any other layer, and verify it.
///
/// Reads the layer count (VIA 11), refuses a layer, row or column outside the keyboard
/// and a keycode outside the allowlist before anything is written, reads the key
/// (`before`), and returns without writing when it already holds `code`. Otherwise one
/// VIA 05 frame is sent (never resent), its echoed keycode must equal `code`, and the key
/// is read back; a different value is `VerifyMismatch`.
pub fn set_keycode<L: RawLink>(
    s: &mut Session<L>,
    layer: u8,
    row: u8,
    col: u8,
    code: u16,
) -> Result<KeycodeOutcome, SessionError> {
    // Reads inside the sequence are sent once: the worst case is one reply timeout per frame.
    s.single_reads(|s| keycode_inner(s, layer, row, col, code))
}

fn keycode_inner<L: RawLink>(
    s: &mut Session<L>,
    layer: u8,
    row: u8,
    col: u8,
    code: u16,
) -> Result<KeycodeOutcome, SessionError> {
    s.set_limits(|p| p.with_matrix(MATRIX_ROWS, MATRIX_COLS));
    let layers = decode_layer_count(&s.request(Request::GetLayerCount)?)?;
    s.set_limits(|p| p.with_layer_count(layers));
    let request = Request::SetKeycode {
        layer,
        row,
        col,
        code,
    };
    // Dry run: range and allowlist checks before any frame of the sequence.
    s.policy_with(Tier::Persistent).check(&request)?;

    let before = read_key(s, layer, row, col)?;
    if before == code {
        return Ok(KeycodeOutcome {
            layer,
            row,
            col,
            before,
            after: before,
            frames_written: 0,
            unchanged: true,
        });
    }
    // Never overwrite a key MyHUB could not put back (MO, LT, MT, Fn, quantum keys).
    if let Err(reason) = check_writable(before) {
        return Err(PolicyError::Invalid {
            field: "current keycode",
            reason,
        }
        .into());
    }
    let policy = s.policy_with(Tier::Persistent);
    let reply = s.request_with(&policy, request)?;
    let echoed = u16::from_be_bytes([reply[4], reply[5]]);
    // The read-back is the real check; it also runs after a wrong echo (no second write).
    let after = read_key(s, layer, row, col)?;
    if echoed != code && after == code {
        return Err(mismatch(code, echoed));
    }
    if after != code {
        return Err(mismatch(code, after));
    }
    Ok(KeycodeOutcome {
        layer,
        row,
        col,
        before,
        after,
        frames_written: 1,
        unchanged: false,
    })
}
