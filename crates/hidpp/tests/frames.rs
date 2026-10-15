mod common;

use common::{frames, hex, table_from_json};
use hidpp::frame::{DEFAULT_SW_ID, FrameError, Message, decode, is_reply_to};
use hidpp::{CheckedRequest, FeatureTable, Policy};

/// Runs a logged TX frame back through the policy gate.
fn checked_from_tx(table: &FeatureTable, raw: &[u8]) -> CheckedRequest {
    let Message::Response {
        device_index,
        feature_index,
        function,
        sw_id,
        payload,
    } = decode(raw).expect("TX decodes")
    else {
        panic!("TX did not decode as a response-shaped frame");
    };
    Policy::new()
        .check_indexed(table, device_index, feature_index, function, sw_id, payload)
        .unwrap_or_else(|e| panic!("TX {raw:02x?} refused: {e}"))
}

fn checked(dev: u8, feat: u8, f: u8, sw: u8) -> CheckedRequest {
    // Feature 9 in the fixture table is 0x8100, function 5 is ReadData.
    Policy::new()
        .check_indexed(&table_from_json(), dev, feat, f, sw, [0; 16])
        .unwrap()
}

#[test]
fn replay_log_counts() {
    let f = frames();
    let tx = f.iter().filter(|(t, _)| *t).count();
    let rx = f.iter().filter(|(t, _)| !*t).count();
    assert_eq!((tx, rx), (165, 165));
    assert_eq!(f.len(), 330);
}

#[test]
fn every_tx_passes_the_gate_and_reencodes_identically_and_every_rx_replies() {
    let table = table_from_json();
    let f = frames();
    let mut last_tx: Option<CheckedRequest> = None;
    let mut pairs = 0;
    for (is_tx, raw) in &f {
        assert_eq!(raw.len(), 20, "all logged frames are long reports");
        assert_eq!(raw[0], 0x11);
        if *is_tx {
            let req = checked_from_tx(&table, raw);
            assert_eq!(req.sw_id(), DEFAULT_SW_ID);
            assert_eq!(req.encode().as_slice(), raw.as_slice(), "re-encode");
            last_tx = Some(req);
        } else {
            let req = last_tx.as_ref().expect("RX preceded by TX");
            let msg = decode(raw).expect("RX decodes");
            assert!(matches!(msg, Message::Response { .. }), "no errors in log");
            assert!(is_reply_to(req, &msg), "RX {raw:02x?} not a reply");
            pairs += 1;
        }
    }
    assert_eq!(pairs, 165);
}

#[test]
fn request_encoding_layout() {
    let mut params = [0u8; 16];
    params[0] = 0xAA;
    params[15] = 0xBB;
    // Index 9 = 0x8100 in the fixture table, fn 0 takes any params.
    let r = Policy::new()
        .check_indexed(&table_from_json(), 1, 9, 0, 0x0B, params)
        .unwrap();
    let e = r.encode();
    assert_eq!(&e[..5], &[0x11, 0x01, 0x09, 0x0B, 0xAA]);
    assert_eq!(e[19], 0xBB);
    assert_eq!(r.params(), &params);
    assert_eq!(
        (r.device_index(), r.feature_index(), r.function(), r.sw_id()),
        (1, 9, 0, 0x0B)
    );
}

#[test]
fn decode_rejects_bad_input() {
    assert_eq!(decode(&[]), Err(FrameError::BadLength(0)));
    assert_eq!(decode(&[0x11; 19]), Err(FrameError::BadLength(19)));
    assert_eq!(decode(&[0x11; 7]), Err(FrameError::BadLength(7)));
    assert_eq!(
        decode(&[0x20; 20]),
        Err(FrameError::UnsupportedReportId(0x20))
    );
    assert_eq!(decode(&[0x10; 21]), Err(FrameError::BadLength(21)));
    assert_eq!(decode(&[0x11; 21]), Err(FrameError::BadLength(21)));
    // Report 0x10 is exactly 7 bytes, 0x11 exactly 20.
    assert_eq!(decode(&[0x10; 20]), Err(FrameError::BadLength(20)));
    assert_eq!(decode(&[0x10; 6]), Err(FrameError::BadLength(6)));
    let mut short20 = [0u8; 20];
    short20[0] = 0x10;
    short20[2] = 0x07;
    short20[3] = 0x2B;
    assert!(decode(&short20).is_err());
    assert!(decode(&[0x10, 0x01, 0x07, 0x2B, 0, 0, 0]).is_ok());
}

#[test]
fn decode_hidpp2_error() {
    let m = decode(&hex("1101ff095b020000000000000000000000000000")).unwrap();
    assert_eq!(
        m,
        Message::Error {
            device_index: 1,
            feature_index: 0x09,
            function: 5,
            sw_id: 0x0B,
            code: 2,
        }
    );
    assert!(is_reply_to(&checked(1, 9, 5, 0x0B), &m));
    assert!(!is_reply_to(&checked(1, 9, 5, 0x0C), &m));
    assert!(!is_reply_to(&checked(1, 9, 0, 0x0B), &m));
    assert!(!is_reply_to(&checked(2, 9, 5, 0x0B), &m));
    assert!(!is_reply_to(&checked(1, 7, 2, 0x0B), &m));
}

#[test]
fn hidpp10_error_replies_to_the_matching_request() {
    // `11 dev 8F featIdx fn|sw err`: here the 20 byte form, device offline.
    let m = decode(&hex("11018f095b090000000000000000000000000000")).unwrap();
    assert!(matches!(
        m,
        Message::Hidpp10 {
            sub_id: 0x8F,
            device_index: 1,
            ..
        }
    ));
    assert!(is_reply_to(&checked(1, 9, 5, 0x0B), &m));
    assert!(!is_reply_to(&checked(1, 9, 5, 0x0C), &m));
    assert!(!is_reply_to(&checked(1, 9, 4, 0x0B), &m));
    assert!(!is_reply_to(&checked(1, 7, 2, 0x0B), &m));
    assert!(!is_reply_to(&checked(2, 9, 5, 0x0B), &m));
    // Short form.
    let s = decode(&[0x10, 0x01, 0x8F, 0x09, 0x5B, 0x09, 0x00]).unwrap();
    assert!(is_reply_to(&checked(1, 9, 5, 0x0B), &s));
    // Other HID++ 1.0 reports never reply.
    let n = decode(&[0x10, 0x01, 0x41, 0x09, 0x5B, 0x09, 0x00]).unwrap();
    assert!(!is_reply_to(&checked(1, 9, 5, 0x0B), &n));
}

#[test]
fn foreign_sw_id_is_not_a_reply() {
    let m = decode(&hex("1101095c000101ff000201ff000300ff000400ff")).unwrap();
    assert!(matches!(m, Message::Response { sw_id: 0x0C, .. }));
    assert!(!is_reply_to(&checked(1, 9, 5, 0x0B), &m));
}

#[test]
fn decode_notification_and_hidpp10() {
    let note = decode(&hex("1101061000ff00000000000000000000000000ff")).unwrap();
    match &note {
        Message::Notification {
            device_index,
            feature_index,
            function,
            payload,
        } => {
            assert_eq!((*device_index, *feature_index, *function), (1, 6, 1));
            assert_eq!(payload[0], 0x00);
            assert_eq!(payload[1], 0xFF);
            assert_eq!(payload[15], 0xFF);
        }
        other => panic!("{other:?}"),
    }
    assert!(!is_reply_to(&checked(1, 6, 1, 2), &note));

    for sub in [0x40u8, 0x41, 0x49, 0x4B, 0x8F] {
        let short = [0x10, 0x01, sub, 0x01, 0x02, 0x03, 0x04];
        match decode(&short).unwrap() {
            Message::Hidpp10 {
                device_index,
                sub_id,
                payload,
            } => {
                assert_eq!((device_index, sub_id), (1, sub));
                assert_eq!(&payload[..4], &[0x01, 0x02, 0x03, 0x04]);
                assert!(payload[4..].iter().all(|&b| b == 0), "zero padded");
            }
            other => panic!("{other:?}"),
        }
    }
    // 0x42 is not a HID++ 1.0 sub id: classified as a HID++ 2.0 frame.
    assert!(matches!(
        decode(&[0x10, 0x01, 0x42, 0x10, 0, 0, 0]).unwrap(),
        Message::Notification { .. }
    ));
}

#[test]
fn short_hidpp2_report_is_zero_padded() {
    let m = decode(&[0x10, 0x01, 0x07, 0x2B, 0x01, 0x02, 0x03]).unwrap();
    match m {
        Message::Response { payload, sw_id, .. } => {
            assert_eq!(sw_id, 0x0B);
            assert_eq!(&payload[..3], &[1, 2, 3]);
            assert!(payload[3..].iter().all(|&b| b == 0));
        }
        other => panic!("{other:?}"),
    }
}
