mod common;

use common::{ack, build, data1, data2, link_with_token};
use mdr::link::{ACK_TIMEOUT_MS, MAX_RESENDS};
use mdr::{
    CheckedRequest, DeviceLimits, FunctionTable, Link, LinkEvent, Policy, SendError, Table, Tick,
    messages,
};

fn checked(link: &Link, req: mdr::Request) -> CheckedRequest {
    Policy::new()
        .check(
            link.epoch(),
            &FunctionTable::new(),
            &DeviceLimits::new(),
            &req,
        )
        .expect("read tier request")
}

fn info(link: &Link) -> CheckedRequest {
    checked(link, messages::protocol_info(Table::One))
}

fn delivered(events: &[LinkEvent]) -> Vec<&mdr::ReceivedFrame> {
    events
        .iter()
        .filter_map(|e| match e {
            LinkEvent::Delivered(f) => Some(f),
            _ => None,
        })
        .collect()
}

fn outgoing(events: &[LinkEvent]) -> Vec<Vec<u8>> {
    events
        .iter()
        .filter_map(|e| match e {
            LinkEvent::Outgoing(b) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn headset_pushes_three_frames_before_our_first_send() {
    let mut link = Link::new();
    let mut bytes = Vec::new();
    bytes.extend(data1(0, &[0x25, 0x09, 0x64, 0x00, 0x64, 0x00, 0x14, 0x14]));
    bytes.extend(data1(1, &[0x25, 0x0A, 0x32, 0x00, 0x15]));
    bytes.extend(data1(0, &[0x15, 0x01, 0x01, 0x01]));
    let ev = link.on_bytes(&bytes, 100);
    // Each frame is ACKed with 1 - theirSeq and delivered.
    assert_eq!(outgoing(&ev), vec![ack(1), ack(0), ack(1)]);
    assert_eq!(delivered(&ev).len(), 3);
    assert_eq!(link.next_seq(), 0);
    assert!(!link.is_busy());

    // Now our first send uses seq 0 and the headset ACK carries seq 1.
    let first = link.send(info(&link), 1000).unwrap();
    assert_eq!(first, data1(0, &[0x00, 0x00]));
    assert_eq!(link.on_bytes(&ack(1), 1010), vec![LinkEvent::Acked]);
    assert_eq!(link.next_seq(), 1);
}

#[test]
fn sequence_alternates_0_1_0() {
    let mut link = Link::new();
    assert_eq!(link.send(info(&link), 0).unwrap(), data1(0, &[0, 0]));
    link.on_bytes(&ack(1), 5);
    assert_eq!(link.send(info(&link), 10).unwrap(), data1(1, &[0, 0]));
    link.on_bytes(&ack(0), 15);
    assert_eq!(link.send(info(&link), 20).unwrap(), data1(0, &[0, 0]));
}

#[test]
fn one_frame_at_a_time() {
    let mut link = Link::new();
    link.send(info(&link), 0).unwrap();
    assert_eq!(link.send(info(&link), 1), Err(SendError::Busy));
    link.on_bytes(&ack(1), 2);
    assert!(link.send(info(&link), 3).is_ok());
}

#[test]
fn resend_after_750_ms_with_the_same_bytes() {
    let mut link = Link::new();
    let first = link.send(info(&link), 1000).unwrap();
    assert_eq!(link.next_deadline(), Some(1000 + ACK_TIMEOUT_MS));
    assert_eq!(link.on_tick(1000 + ACK_TIMEOUT_MS - 1), None);
    assert_eq!(
        link.on_tick(1000 + ACK_TIMEOUT_MS),
        Some(Tick::Resend(first.clone()))
    );
    // The timer restarts from the resend.
    assert_eq!(link.next_deadline(), Some(1000 + 2 * ACK_TIMEOUT_MS));
    // A late ACK still completes the exchange.
    assert_eq!(link.on_bytes(&ack(1), 1800), vec![LinkEvent::Acked]);
    assert_eq!(link.next_deadline(), None);
    assert_eq!(link.on_tick(99_999), None);
}

#[test]
fn link_dead_after_ten_resends() {
    let mut link = Link::new();
    let first = link.send(info(&link), 0).unwrap();
    let mut now = 0;
    for _ in 0..MAX_RESENDS {
        now += ACK_TIMEOUT_MS;
        assert_eq!(link.on_tick(now), Some(Tick::Resend(first.clone())));
    }
    now += ACK_TIMEOUT_MS;
    assert_eq!(link.on_tick(now), Some(Tick::LinkDead));
    assert!(link.is_dead());
    assert_eq!(link.send(info(&link), now), Err(SendError::Dead));
    assert_eq!(link.on_tick(now + 10_000), None);
}

#[test]
fn ack_with_our_own_seq_is_ignored() {
    let mut link = Link::new();
    link.send(info(&link), 0).unwrap();
    // We sent seq 0, an ACK carrying seq 0 is invalid.
    assert!(link.on_bytes(&ack(0), 1).is_empty());
    assert!(link.is_busy());
    assert_eq!(link.next_seq(), 0);
    // An ACK with nothing outstanding is ignored as well.
    let mut idle = Link::new();
    assert!(idle.on_bytes(&ack(1), 1).is_empty());
    assert_eq!(idle.next_seq(), 0);
}

#[test]
fn duplicate_is_acked_but_not_delivered() {
    let mut link = Link::new();
    let f = data1(0, &[0x15, 0x01, 0x01, 0x01]);
    let first = link.on_bytes(&f, 0);
    assert_eq!(delivered(&first).len(), 1);
    let again = link.on_bytes(&f, 750);
    assert_eq!(outgoing(&again), vec![ack(1)]);
    assert!(delivered(&again).is_empty());
    // The next frame (seq 1) is new.
    let next = link.on_bytes(&data1(1, &[0x01, 0x02]), 800);
    assert_eq!(delivered(&next).len(), 1);
}

#[test]
fn frames_we_cannot_parse_and_table_2_frames_are_acked_and_delivered() {
    let mut link = Link::new();
    let ev = link.on_bytes(&data1(1, &[0xEE, 0x01, 0x02]), 0);
    assert_eq!(outgoing(&ev), vec![ack(0)]);
    assert_eq!(delivered(&ev).len(), 1);
    let ev = link.on_bytes(&data2(0, &[0x07, 0x00, 0x00]), 0);
    assert_eq!(outgoing(&ev), vec![ack(1)]);
    assert_eq!(delivered(&ev).len(), 1);
}

#[test]
fn shot_frames_are_delivered_without_an_ack() {
    let mut link = Link::new();
    let ev = link.on_bytes(&build(0x1C, 0, &[0x01]), 0);
    assert!(outgoing(&ev).is_empty());
    assert_eq!(delivered(&ev).len(), 1);
}

#[test]
fn bytes_may_arrive_in_pieces() {
    let mut link = Link::new();
    let f = data1(0, &[0x25, 0x0A, 0x32, 0x00, 0x15]);
    let (a, b) = f.split_at(4);
    assert!(link.on_bytes(a, 0).is_empty());
    let ev = link.on_bytes(b, 1);
    assert_eq!(outgoing(&ev), vec![ack(1)]);
}

#[test]
fn reset_returns_to_seq_0_and_forgets_the_last_received_seq() {
    let mut link = Link::new();
    link.send(info(&link), 0).unwrap();
    link.on_bytes(&ack(1), 1);
    link.on_bytes(&data1(0, &[0x01, 0x00]), 2);
    assert_eq!(link.next_seq(), 1);
    // Dead link and half a frame in the buffer are cleared too.
    link.send(info(&link), 3).unwrap();
    link.on_bytes(&[0x3E, 0x0C], 3);
    link.reset();
    assert_eq!(link.next_seq(), 0);
    assert!(!link.is_busy() && !link.is_dead());
    assert_eq!(link.next_deadline(), None);
    // seq 0 from the headset is new again, not a duplicate.
    let ev = link.on_bytes(&data1(0, &[0x01, 0x00]), 10);
    assert_eq!(delivered(&ev).len(), 1);
    assert_eq!(link.send(info(&link), 11).unwrap(), data1(0, &[0, 0]));
}

#[test]
fn alert_token_from_before_a_reset_is_stale() {
    let (mut link, token) = link_with_token(0x07, 0x01);
    let token = token.expect("token");
    let answer = Policy::new()
        .with_disconnecting_changes()
        .check_alert_answer(token, true)
        .unwrap();
    link.reset();
    assert_eq!(link.send(answer, 0), Err(SendError::StaleAlert));
    assert!(!link.is_busy());
}

#[test]
fn alert_token_from_the_current_epoch_sends() {
    let (mut link, token) = link_with_token(0x07, 0x01);
    let answer = Policy::new()
        .with_disconnecting_changes()
        .check_alert_answer(token.unwrap(), true)
        .unwrap();
    assert_eq!(
        link.send(answer, 0).unwrap(),
        data1(0, &[0x98, 0x00, 0x07, 0x01])
    );
}

#[test]
fn alert_token_from_another_link_is_stale() {
    let (_, token) = link_with_token(0x07, 0x01);
    let answer = Policy::new()
        .with_setting_changes()
        .check_alert_answer(token.unwrap(), false)
        .unwrap();
    let mut other = Link::new();
    assert_eq!(other.send(answer, 0), Err(SendError::StaleAlert));
}

#[test]
fn reset_changes_the_epoch_of_delivered_frames() {
    let mut link = Link::new();
    let e0 = link.epoch();
    let ev = link.on_bytes(&data1(0, &[0x01, 0x00]), 0);
    assert_eq!(delivered(&ev)[0].epoch(), e0);
    link.reset();
    assert_ne!(link.epoch(), e0);
}

#[test]
fn request_checked_before_a_reset_is_refused() {
    let mut link = Link::new();
    let request = info(&link);
    link.reset();
    assert_eq!(link.send(request, 0), Err(SendError::StaleRequest));
    assert!(!link.is_busy());
}
