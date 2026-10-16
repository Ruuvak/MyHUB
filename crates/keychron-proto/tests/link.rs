//! Receiver decoders on the replies seen on hardware (receiver guide, 2026-10-15).

use keychron_proto::link::{decode_link_firmware, decode_link_info, decode_link_state};
use keychron_proto::{DecodeError, Policy, PolicyError, Request, Target};

fn frame(bytes: &[u8]) -> [u8; 32] {
    let mut f = [0u8; 32];
    f[..bytes.len()].copy_from_slice(bytes);
    f
}

#[test]
fn protocol_reply_decodes() {
    let info = decode_link_info(&frame(&[0xB1, 0x01, 0x00, 0x08])).unwrap();
    assert_eq!(info.version, 1);
    assert_eq!(info.flags.raw, 0x08);
    assert!(info.flags.via_disable_gamepad_input);
    assert!(!info.flags.state_notify_over_via);
    assert!(!info.flags.multi_device);
    assert!(!info.flags.mouse_driver_over_via);
    assert_eq!(info.byte4, 0);
    let old = decode_link_info(&frame(&[0xB1, 0x01, 0x00, 0x00])).unwrap();
    assert!(!old.flags.via_disable_gamepad_input);
}

#[test]
fn state_reply_decodes_connected_and_disconnected() {
    let up = decode_link_state(&frame(&[0xB2, 0x01, 0x34, 0x34, 0x20, 0x0E, 0x01])).unwrap();
    assert_eq!((up.linked_vid, up.linked_pid), (0x3434, 0x0E20));
    assert!(up.connected);
    assert_eq!(up.raw_byte1, 1);
    assert!(up.is_k2he());

    let down = decode_link_state(&frame(&[0xB2, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00])).unwrap();
    assert!(!down.connected);
    assert!(!down.is_k2he());

    // Linked but another device, or the K2 HE flagged not connected.
    let other = decode_link_state(&frame(&[0xB2, 0x01, 0x34, 0x34, 0x21, 0x0E, 0x01])).unwrap();
    assert!(other.connected && !other.is_k2he());
    let asleep = decode_link_state(&frame(&[0xB2, 0x01, 0x34, 0x34, 0x20, 0x0E, 0x00])).unwrap();
    assert!(!asleep.is_k2he());
}

#[test]
fn firmware_reply_skips_zero_bytes() {
    let text = b"d.3.1 Jan 5 2026 16:57:08";
    let mut bytes = vec![0xB3];
    bytes.extend_from_slice(text);
    assert_eq!(
        decode_link_firmware(&frame(&bytes)).unwrap(),
        "d.3.1 Jan 5 2026 16:57:08"
    );
}

#[test]
fn wrong_command_byte_is_refused() {
    assert!(matches!(
        decode_link_state(&frame(&[0xB1, 0x01])),
        Err(DecodeError::WrongReply { index: 0, .. })
    ));
}

#[test]
fn receiver_requests_pass_the_read_policy_and_others_do_not() {
    let p = Policy::read_only();
    for (bytes, request) in [
        ([0xB1u8], Request::LinkProtocol),
        ([0xB2], Request::LinkState),
        ([0xB3], Request::LinkFirmware),
    ] {
        let checked = p.check_bytes(&bytes).unwrap();
        assert_eq!(checked.encode(), frame(&bytes));
        assert_eq!(checked.request().target(), Target::Receiver);
        assert_eq!(*checked.request(), request);
    }
    for command in [0xB5u8, 0xBC] {
        assert_eq!(
            p.check_bytes(&[command]),
            Err(PolicyError::NotAllowlisted { command, sub: 0 })
        );
    }
    for command in [0xB1u8, 0xB2, 0xB3] {
        assert_eq!(
            p.check_bytes(&[command, 0x00, 0x01]),
            Err(PolicyError::TrailingBytes { command })
        );
    }
    assert_eq!(
        p.check_bytes(&[0xBA]),
        Err(PolicyError::Forbidden {
            command: 0xBA,
            sub: 0
        })
    );
}
