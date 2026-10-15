//! Shared helpers. Nothing here opens a device. Frames and buffers are built by hand,
//! independently of the crate, so tests do not check the encoder against itself.
#![allow(dead_code)]

use keychron_proto::Frame;

pub const READ: &str = include_str!("../fixtures/k2he_read.txt");
pub const KEYMAP: &str = include_str!("../fixtures/k2he_keymap.txt");
pub const HE: &str = include_str!("../fixtures/k2he_he.txt");

pub fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
        .collect()
}

pub fn pad(bytes: &[u8]) -> Frame {
    let mut f = [0u8; 32];
    f[..bytes.len()].copy_from_slice(bytes);
    f
}

/// Every `request => reply` line: (request bytes, reply frame).
pub fn read_pairs() -> Vec<(Vec<u8>, Frame)> {
    READ.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (req, rep) = l.split_once("=>").expect("=> separator");
            (hex(req), pad(&hex(rep)))
        })
        .collect()
}

/// The golden reply for a request given as hex, e.g. `"a9 10"`.
pub fn reply_for(request: &str) -> Frame {
    let want = hex(request);
    read_pairs()
        .into_iter()
        .find(|(r, _)| *r == want)
        .unwrap_or_else(|| panic!("no fixture line for {request}"))
        .1
}

/// One keymap row of the fixture.
pub fn keymap_row(layer: u8, row: u8) -> Vec<u16> {
    let key = format!("L{layer} R{row}:");
    let line = KEYMAP
        .lines()
        .find(|l| l.starts_with(&key))
        .unwrap_or_else(|| panic!("no keymap line {key}"));
    line[key.len()..]
        .split_whitespace()
        .map(|w| u16::from_str_radix(w, 16).expect("keycode"))
        .collect()
}

/// A fact line of `k2he_he.txt` as bytes.
pub fn he_fact(name: &str) -> Vec<u8> {
    let key = format!("{name}:");
    let line = HE
        .lines()
        .find(|l| l.starts_with(&key))
        .unwrap_or_else(|| panic!("no HE fact {name}"));
    hex(&line[key.len()..])
}

/// The reply a keyboard would give for `12 off_hi off_lo len` over a flat keymap
/// image (big-endian keycodes), built by hand.
pub fn keymap_reply(image: &[u8], offset: usize, len: usize) -> Frame {
    let mut f = [0u8; 32];
    f[0] = 0x12;
    f[1] = (offset >> 8) as u8;
    f[2] = offset as u8;
    f[3] = len as u8;
    f[4..4 + len].copy_from_slice(&image[offset..offset + len]);
    f
}

/// Read policy with every runtime size the K2 HE reports (A9 10, VIA 0D, VIA 11, A8 05,
/// definition matrix).
pub fn sized(policy: keychron_proto::Policy) -> keychron_proto::Policy {
    policy
        .with_profile_count(3)
        .with_profile_buffer_size(860)
        .with_macro_buffer_size(2027)
        .with_layer_count(4)
        .with_matrix(6, 16)
        .with_led_count(84)
}
