mod common;

use common::{ack, build, data1};
use mdr::{DataType, FrameDecoder, FrameError, decode_frame, encode_ack};

#[test]
fn ack_frames_match_the_app_constants() {
    assert_eq!(encode_ack(0), common::hex("3e 01 00 00 00 00 00 01 3c"));
    assert_eq!(encode_ack(1), common::hex("3e 01 01 00 00 00 00 02 3c"));
    assert_eq!(encode_ack(0), ack(0));
    assert_eq!(encode_ack(1), ack(1));
}

#[test]
fn decode_checks_checksum_length_and_delimiters() {
    let wire = data1(1, &[0x25, 0x0A, 0x32, 0x00, 0x15]);
    let f = decode_frame(&wire).expect("valid");
    assert_eq!(f.data_type, DataType::DataMdr);
    assert_eq!(f.seq, 1);
    assert_eq!(f.payload, [0x25, 0x0A, 0x32, 0x00, 0x15]);

    let mut bad = wire.clone();
    let n = bad.len();
    bad[n - 2] ^= 0x01;
    assert_eq!(decode_frame(&bad), Err(FrameError::BadChecksum));

    assert_eq!(decode_frame(&wire[1..]), Err(FrameError::NotDelimited));

    // Length field says 6, one payload byte present, checksum fixed up.
    let mut body = vec![0x0C, 0x00, 0, 0, 0, 6, 0x11];
    let sum = body.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    body.push(sum);
    let mut w = vec![0x3E];
    w.extend(body);
    w.push(0x3C);
    assert_eq!(decode_frame(&w), Err(FrameError::BadLength));
}

#[test]
fn escaped_bytes_round_trip_through_the_decoder() {
    let payload = [0x3C, 0x3D, 0x3E, 0x2C, 0x2D, 0x2E];
    let wire = build(0x0C, 0, &payload);
    let inner = &wire[1..wire.len() - 1];
    assert!(!inner.contains(&0x3C) && !inner.contains(&0x3E));
    let mut dec = FrameDecoder::new();
    let frames = dec.push(&wire);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].payload, payload);
}

fn stream() -> (Vec<u8>, Vec<Vec<u8>>) {
    let payloads: Vec<Vec<u8>> = vec![
        vec![0x25, 0x09, 0x64, 0x00, 0x64, 0x00, 0x14, 0x14],
        vec![0x3C, 0x3D, 0x3E],
        vec![0x15, 0x01, 0x01, 0x01],
    ];
    let mut bytes = Vec::new();
    for (i, p) in payloads.iter().enumerate() {
        bytes.extend(data1(i as u8 & 1, p));
    }
    (bytes, payloads)
}

#[test]
fn decoder_handles_a_split_at_every_byte_boundary() {
    let (bytes, payloads) = stream();
    for cut in 0..=bytes.len() {
        let mut dec = FrameDecoder::new();
        let mut got = dec.push(&bytes[..cut]);
        got.extend(dec.push(&bytes[cut..]));
        let p: Vec<_> = got.into_iter().map(|f| f.payload).collect();
        assert_eq!(p, payloads, "cut at {cut}");
    }
}

#[test]
fn decoder_handles_one_byte_at_a_time_and_all_at_once() {
    let (bytes, payloads) = stream();
    let mut dec = FrameDecoder::new();
    let mut got = Vec::new();
    for b in &bytes {
        got.extend(dec.push(&[*b]));
    }
    assert_eq!(
        got.into_iter().map(|f| f.payload).collect::<Vec<_>>(),
        payloads
    );
    let mut dec = FrameDecoder::new();
    let got = dec.push(&bytes);
    assert_eq!(got.len(), 3);
    assert_eq!(dec.dropped(), 0);
}

#[test]
fn garbage_between_frames_is_skipped() {
    let (_, payloads) = stream();
    let mut bytes = vec![0x00, 0xFF, 0x3C, 0x41];
    bytes.extend(data1(0, &payloads[0]));
    bytes.extend([0x13, 0x37, 0x3C, 0x3C]);
    bytes.extend(data1(1, &payloads[2]));
    bytes.extend([0xAA]);
    let mut dec = FrameDecoder::new();
    let got: Vec<_> = dec.push(&bytes).into_iter().map(|f| f.payload).collect();
    assert_eq!(got, vec![payloads[0].clone(), payloads[2].clone()]);
}

#[test]
fn bad_checksum_is_dropped_and_the_next_frame_still_decodes() {
    let good = data1(1, &[0x15, 0x01, 0x01, 0x01]);
    let mut bad = data1(0, &[0x25, 0x0A, 0x32, 0x00, 0x15]);
    let n = bad.len();
    bad[n - 2] = bad[n - 2].wrapping_add(1);
    let mut bytes = bad;
    bytes.extend(&good);
    let mut dec = FrameDecoder::new();
    let got = dec.push(&bytes);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].payload, [0x15, 0x01, 0x01, 0x01]);
    assert_eq!(dec.dropped(), 1);
}

#[test]
fn bad_escape_and_truncated_frame_resync_on_the_next_start_byte() {
    let good = data1(0, &[0x01, 0x00]);
    let mut bytes = vec![0x3E, 0x0C, 0x3D, 0x41, 0x3C]; // bad escape
    bytes.extend([0x3E, 0x0C, 0x00, 0x00]); // truncated, cut by the next start byte
    bytes.extend(&good);
    let mut dec = FrameDecoder::new();
    let got = dec.push(&bytes);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].payload, [0x01, 0x00]);
    assert_eq!(dec.dropped(), 2);
}

#[test]
fn oversized_frame_is_dropped_and_the_buffer_stays_bounded() {
    let mut bytes = vec![0x3E];
    bytes.extend(std::iter::repeat_n(0x11, 10_000));
    bytes.extend(data1(1, &[0x01, 0x00]));
    let mut dec = FrameDecoder::new();
    let got = dec.push(&bytes);
    // The oversized frame is dropped. After the drop the decoder waits for a start byte,
    // and the good frame that follows decodes.
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].payload, [0x01, 0x00]);
    assert!(dec.dropped() >= 1);
}

#[test]
fn non_data_types_decode_but_are_not_acked() {
    let w = build(0x1C, 0, &[0x01]);
    let f = decode_frame(&w).unwrap();
    assert_eq!(f.data_type, DataType::ShotMdr);
    assert!(!f.data_type.needs_ack());
    assert_eq!(DataType::from_u8(0x7F), DataType::Unknown(0x7F));
}
