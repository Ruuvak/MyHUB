mod common;

use common::he_fact;
use keychron_proto::DecodeError;
use keychron_proto::he::{AdvanceMode, KeyMode, ProfileBuffer, SnapType};

const ROWS: u8 = 6;
const COLS: u8 = 16;
const DKS: u8 = 20;
const SNAP: u8 = 20;

fn key_offset(row: usize, col: usize) -> usize {
    4 + 4 * (row * 16 + col)
}

/// 860-byte buffer built from the golden facts: global word of profile 0, key (3,1)
/// bound to DKS slot 0 with the T1 slot bytes, key (2,2) analog id 3, one snap pair,
/// the name "AB".
fn buffer() -> Vec<u8> {
    let mut b = vec![0u8; 860];
    b[..4].copy_from_slice(&he_fact("global_profile0"));
    let dks_key = key_offset(3, 1);
    b[dks_key..dks_key + 4].copy_from_slice(&he_fact("dks_t1_keyword"));
    let analog = key_offset(2, 2);
    b[analog..analog + 4].copy_from_slice(&he_fact("analog_id3_keyword"));
    // DKS table starts at 388, snap table at 768, name at 828.
    b[388..388 + 19].copy_from_slice(&he_fact("dks_t1_slot0"));
    // Snap pair A (3,1) and D (3,3): col in bits 7..3, row in bits 2..0, type 2.
    b[768..771].copy_from_slice(&[(1 << 3) | 3, (3 << 3) | 3, 2]);
    b[828..832].copy_from_slice(&he_fact("name_ab"));
    b
}

#[test]
fn layout_size_is_860_for_the_k2_he() {
    assert_eq!(ProfileBuffer::layout_size(ROWS, COLS, 20, 20), 860);
}

#[test]
fn golden_global_word_decodes() {
    let p = ProfileBuffer::parse(&buffer(), ROWS, COLS, DKS, SNAP).unwrap();
    assert_eq!(p.global.raw, 0x0001_0451);
    assert_eq!(p.global.mode, KeyMode::Static);
    assert_eq!(
        (p.global.distance, p.global.press, p.global.release),
        (20, 4, 4)
    );
    assert_eq!(p.global.advance_mode, AdvanceMode::None);
    // Profile 1 (Gaming) is the same word with mode 2.
    let mut gaming = buffer();
    gaming[..4].copy_from_slice(&he_fact("global_profile1"));
    let g = ProfileBuffer::parse(&gaming, ROWS, COLS, DKS, SNAP).unwrap();
    assert_eq!(g.global.mode, KeyMode::RapidTrigger);
}

#[test]
fn keys_dks_snap_and_name_parse() {
    let p = ProfileBuffer::parse(&buffer(), ROWS, COLS, DKS, SNAP).unwrap();
    assert_eq!(p.keys.len(), 96);

    let analog = p.key(2, 2).unwrap();
    assert_eq!(analog.advance_mode, AdvanceMode::Analog);
    assert_eq!(analog.index, 3);
    assert_eq!(analog.mode, KeyMode::Global);

    let dks_key = p.key(3, 1).unwrap();
    assert_eq!(dks_key.raw, 0x0030_0000);
    assert_eq!(dks_key.advance_mode, AdvanceMode::Dks);
    assert_eq!(dks_key.index, 0);
    assert!(p.key(6, 0).is_none() && p.key(0, 16).is_none());

    let slot = &p.dks[0];
    assert!(slot.used);
    assert_eq!(slot.travel, [11, 22, 33, 44]);
    assert_eq!(slot.keycodes, [4, 5, 6, 7]);
    assert_eq!(slot.actions[0], [2, 0, 0, 0]);
    assert_eq!(slot.actions[3], [7, 0, 0, 0]);
    let owner = slot.owner.unwrap();
    assert_eq!((owner.row, owner.col), (3, 1));
    assert!(p.dks[1..].iter().all(|s| !s.used && s.owner.is_none()));
    assert_eq!(p.dks.len(), 20);

    let pair = &p.snap[0];
    assert!(pair.used);
    assert_eq!((pair.key1.row, pair.key1.col), (3, 1));
    assert_eq!((pair.key2.row, pair.key2.col), (3, 3));
    assert_eq!(pair.kind, SnapType::SnapClick);
    assert!(p.snap[1..].iter().all(|s| !s.used));
    assert_eq!(p.snap.len(), 20);

    assert_eq!(p.name, "AB");
}

#[test]
fn layout_mismatch_is_an_error() {
    let mut short = buffer();
    short.pop();
    assert_eq!(
        ProfileBuffer::parse(&short, ROWS, COLS, DKS, SNAP).unwrap_err(),
        DecodeError::LayoutMismatch {
            expected: 860,
            got: 859
        }
    );
    // Right bytes, wrong slot counts.
    assert_eq!(
        ProfileBuffer::parse(&buffer(), ROWS, COLS, DKS, SNAP - 1).unwrap_err(),
        DecodeError::LayoutMismatch {
            expected: 857,
            got: 860
        }
    );
    assert!(ProfileBuffer::parse(&[], ROWS, COLS, DKS, SNAP).is_err());
}

#[test]
fn name_stops_at_the_first_zero_pair() {
    let mut b = buffer();
    // "AB" then a stray unit after the terminator must not leak in.
    b[832..836].copy_from_slice(&[0, 0, 0x43, 0x00]);
    let p = ProfileBuffer::parse(&b, ROWS, COLS, DKS, SNAP).unwrap();
    assert_eq!(p.name, "AB");
}
