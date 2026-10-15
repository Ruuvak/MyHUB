//! Receiver matching, over plain structs. No hidapi enumeration, no device.

use std::ffi::CString;

use myhub_transport::{HidCandidate, RECEIVER_PIDS, UsageRule, select_receivers};

fn cand(
    vid: u16,
    pid: u16,
    interface: i32,
    usage_page: u16,
    usage: u16,
    path: &str,
) -> HidCandidate {
    HidCandidate {
        vid,
        pid,
        interface,
        usage_page,
        usage,
        path: CString::new(path).unwrap(),
    }
}

fn receiver(interface: i32, usage_page: u16, usage: u16, path: &str) -> HidCandidate {
    cand(0x046D, 0xC547, interface, usage_page, usage, path)
}

#[test]
fn linux_style_single_entry_is_accepted_whatever_its_usage() {
    for usage in [0x0001, 0x0002, 0x0006] {
        let entries = [receiver(2, 0xFF00, usage, "/dev/hidraw3")];
        let found = select_receivers(&entries, UsageRule::AnyUsage);
        assert_eq!(found.len(), 1, "usage {usage:#x}");
        assert_eq!(found[0].path().to_str().unwrap(), "/dev/hidraw3");
    }
    // The Windows rule would reject a usage 1 entry.
    let entries = [receiver(2, 0xFF00, 0x0001, "/dev/hidraw3")];
    assert!(select_receivers(&entries, UsageRule::LongReportCollection).is_empty());
}

#[test]
fn duplicate_paths_are_listed_once() {
    let entries = [
        receiver(2, 0xFF00, 0x0001, "/dev/hidraw3"),
        receiver(2, 0xFF00, 0x0002, "/dev/hidraw3"),
        receiver(2, 0xFF00, 0x0001, "/dev/hidraw7"),
    ];
    let found = select_receivers(&entries, UsageRule::AnyUsage);
    let paths: Vec<_> = found.iter().map(|r| r.path().to_str().unwrap()).collect();
    assert_eq!(paths, ["/dev/hidraw3", "/dev/hidraw7"]);
}

#[test]
fn wrong_vid_pid_interface_or_usage_page_is_rejected() {
    let entries = [
        cand(0x046E, 0xC547, 2, 0xFF00, 0x0002, "wrong-vid"),
        cand(0x046D, 0xC548, 2, 0xFF00, 0x0002, "wrong-pid"),
        cand(0x046D, 0xC547, 1, 0xFF00, 0x0002, "wrong-interface"),
        cand(0x046D, 0xC547, 2, 0xFF01, 0x0002, "wrong-usage-page"),
        cand(0x046D, 0xC547, -1, 0xFF00, 0x0002, "no-interface"),
        // A different Logitech device: the mouse itself over USB.
        cand(0x046D, 0xC09B, 2, 0xFF00, 0x0002, "other-device"),
    ];
    for rule in [UsageRule::LongReportCollection, UsageRule::AnyUsage] {
        assert!(select_receivers(&entries, rule).is_empty(), "{rule:?}");
    }
}

#[test]
fn supported_receiver_list_is_the_one_documented_pid() {
    assert_eq!(RECEIVER_PIDS, &[0xC547]);
}

#[test]
fn debug_output_never_contains_the_path() {
    let entries = [receiver(2, 0xFF00, 0x0002, "secret-instance-id")];
    let found = select_receivers(&entries, UsageRule::LongReportCollection);
    assert!(!format!("{found:?}").contains("secret-instance-id"));
    assert!(!format!("{entries:?}").contains("secret-instance-id"));
}

/// Windows-style path of one collection of a receiver interface. `parent` is
/// the shared part of the instance ID, `col` the collection number.
fn win_path(pid: u16, parent: &str, col: u8) -> String {
    format!(
        r"\\?\hid#vid_046d&pid_{pid:04x}&mi_02&col{col:02}#7&{parent}&0&{:04}#{{4d1e55b2-f16f-11cf-88cb-001111000030}}",
        col - 1
    )
}

#[test]
fn windows_style_entries_pick_the_long_collection_and_pair_the_short_one() {
    let col01 = win_path(0xC547, "2a3b4c5d", 1);
    let col02 = win_path(0xC547, "2a3b4c5d", 2);
    let entries = [
        receiver(2, 0xFF00, 0x0001, &col01),
        receiver(2, 0xFF00, 0x0002, &col02),
        // Mouse and keyboard interfaces of the same receiver.
        receiver(0, 0x0001, 0x0002, "win-mi00"),
        receiver(1, 0x0001, 0x0006, "win-mi01"),
    ];
    let found = select_receivers(&entries, UsageRule::LongReportCollection);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].usage, 0x0002);
    assert_eq!(found[0].pid, 0xC547);
    assert_eq!(found[0].path().to_str().unwrap(), col02);
    assert_eq!(found[0].short_path().unwrap().to_str().unwrap(), col01);
}

#[test]
fn short_collection_is_paired_per_receiver() {
    // Two receivers, collections listed interleaved.
    let a01 = win_path(0xC547, "aaaaaaaa", 1);
    let a02 = win_path(0xC547, "aaaaaaaa", 2);
    let b01 = win_path(0xC547, "bbbbbbbb", 1);
    let b02 = win_path(0xC547, "bbbbbbbb", 2);
    let entries = [
        receiver(2, 0xFF00, 0x0001, &b01),
        receiver(2, 0xFF00, 0x0002, &a02),
        receiver(2, 0xFF00, 0x0001, &a01),
        receiver(2, 0xFF00, 0x0002, &b02),
    ];
    let found = select_receivers(&entries, UsageRule::LongReportCollection);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].path().to_str().unwrap(), a02);
    assert_eq!(found[0].short_path().unwrap().to_str().unwrap(), a01);
    assert_eq!(found[1].path().to_str().unwrap(), b02);
    assert_eq!(found[1].short_path().unwrap().to_str().unwrap(), b01);
}

#[test]
fn missing_or_foreign_short_collection_gives_none() {
    let col02 = win_path(0xC547, "2a3b4c5d", 2);
    // No COL01 at all.
    let only_long = [receiver(2, 0xFF00, 0x0002, &col02)];
    let found = select_receivers(&only_long, UsageRule::LongReportCollection);
    assert_eq!(found.len(), 1);
    assert!(found[0].short_path().is_none());

    // A usage 1 collection of another receiver, of another interface and of
    // another usage page is not this receiver's short collection.
    let other = win_path(0xC547, "99999999", 1);
    let entries = [
        receiver(2, 0xFF00, 0x0002, &col02),
        receiver(2, 0xFF00, 0x0001, &other),
        receiver(1, 0xFF00, 0x0001, &win_path(0xC547, "2a3b4c5d", 1)),
        receiver(2, 0xFF01, 0x0001, &win_path(0xC547, "2a3b4c5d", 1)),
    ];
    let found = select_receivers(&entries, UsageRule::LongReportCollection);
    assert_eq!(found.len(), 1);
    assert!(found[0].short_path().is_none());
}

#[test]
fn linux_rule_never_has_a_short_collection() {
    let entries = [
        receiver(2, 0xFF00, 0x0001, "/dev/hidraw3"),
        receiver(2, 0xFF00, 0x0001, "/dev/hidraw4"),
    ];
    let found = select_receivers(&entries, UsageRule::AnyUsage);
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|r| r.short_path().is_none()));
}
