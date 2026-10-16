mod common;

use common::Mock;
use keychron_k2he::{
    Event, KeyboardCache, Session, Snapshot, cache_of, cache_path, read_snapshot, refresh_state,
    to_detail, to_receiver_detail,
};
use myhub_core::{Connection, ConnectionPath, DeviceKind, HeModeView};

fn golden_snapshot() -> (Session<Mock>, Snapshot) {
    let mut s = Session::with_link(Mock::golden());
    let snap = read_snapshot(&mut s).unwrap();
    (s, snap)
}

#[test]
fn profiles_carry_mode_and_millimetres_from_the_global_word() {
    let (_, snap) = golden_snapshot();
    let detail = to_detail(&snap, true, false);
    assert_eq!(detail.summary.id, "keychron-k2he");
    assert_eq!(detail.summary.name, "K2 HE");
    assert_eq!(detail.summary.kind, DeviceKind::Keyboard);
    assert_eq!(detail.summary.connection, Connection::Connected);
    assert_eq!(detail.summary.battery_percent, None);
    assert_eq!(detail.protocol, "VIA 12 / Keychron HE 4");
    assert_eq!(detail.illustration.as_deref(), Some("k2he"));
    assert!(detail.read_only);
    let kb = detail.keyboard.as_ref().unwrap();
    assert_eq!((kb.via_protocol, kb.layer_count), (12, 4));
    assert_eq!(kb.default_layer, Some(2));
    assert!(!kb.other_configurator);
    let he = kb.he.as_ref().unwrap();
    assert_eq!(he.current_profile, 0);
    assert_eq!(he.profiles.len(), 3);
    let p0 = &he.profiles[0];
    assert_eq!(p0.mode, HeModeView::Static);
    assert_eq!(p0.name, None);
    assert_eq!(
        (p0.actuation_mm, p0.press_mm, p0.release_mm),
        (2.0, 0.4, 0.4)
    );
    assert_eq!(p0.per_key_overrides, 0);
    assert_eq!(he.profiles[1].mode, HeModeView::RapidTrigger);
    assert_eq!(he.profiles[1].name.as_deref(), Some("Gaming"));
    assert_eq!(he.curve.as_ref().unwrap()[3], [40, 127]);
    let lighting = kb.lighting.unwrap();
    assert_eq!(
        (lighting.effect, lighting.effect_max, lighting.led_count),
        (5, 24, 84)
    );
    assert!(lighting.per_key);
    let timers = kb.timers.unwrap();
    assert_eq!(timers.backlight_off_s, 600);
    let json = serde_json::to_value(&detail).unwrap();
    assert_eq!(
        json["keyboard"]["he"]["profiles"][1]["mode"],
        "rapidTrigger"
    );
    assert_eq!(json["keyboard"]["otherConfigurator"], false);
    assert_eq!(json["keyboard"]["lighting"]["effectMax"], 24);
}

#[test]
fn keymap_skips_slots_that_are_empty_on_every_layer_and_labels_codes() {
    let (_, snap) = golden_snapshot();
    let detail = to_detail(&snap, true, false);
    let layers = detail.keyboard.unwrap().keymap.unwrap();
    assert_eq!(layers.len(), 4);
    // Row 1 column 15 reads 0000 on every layer: no key there.
    for layer in &layers {
        assert!(!layer.keys.iter().any(|k| (k.row, k.col) == (1, 15)));
    }
    // Every layer lists the same physical slots, fewer than the 96 matrix cells.
    assert!(layers[0].keys.len() < 96);
    assert!(layers.iter().all(|l| l.keys.len() == layers[0].keys.len()));
    // Win layer (2), r5c10 is MO(3).
    let key = layers[2]
        .keys
        .iter()
        .find(|k| (k.row, k.col) == (5, 10))
        .unwrap();
    assert_eq!(key.label, "MO(3)");
    assert_eq!(key.code, 0x5223);
}

#[test]
fn disconnected_and_foreign_traffic_flags_follow_the_arguments() {
    let (_, snap) = golden_snapshot();
    let detail = to_detail(&snap, false, true);
    assert_eq!(detail.summary.connection, Connection::Disconnected);
    assert!(detail.keyboard.unwrap().other_configurator);
}

#[test]
fn refresh_state_patches_profile_and_lighting_with_read_requests_only() {
    let (mut s, mut snap) = golden_snapshot();
    snap.profiles_info.current = 2;
    snap.light.effect = Some(9);
    snap.light.brightness = Some(1);
    let before = s.frames_sent();
    refresh_state(&mut s, &mut snap).unwrap();
    // A9 10 plus four lighting reads.
    assert_eq!(s.frames_sent() - before, 5);
    assert_eq!(snap.profiles_info.current, 0);
    assert_eq!(snap.light.effect, Some(5));
    assert_eq!(snap.light.brightness, Some(0xFF));
}

#[test]
fn events_patch_the_cached_snapshot() {
    let (_, mut snap) = golden_snapshot();
    assert!(snap.apply_event(Event::ProfileChanged { profile: 1 }));
    assert!(!snap.apply_event(Event::ProfileChanged { profile: 1 }));
    assert!(snap.apply_event(Event::EffectChanged { effect: 7 }));
    let detail = to_detail(&snap, true, false);
    let kb = detail.keyboard.unwrap();
    assert_eq!(kb.he.unwrap().current_profile, 1);
    assert_eq!(kb.lighting.unwrap().effect, 7);
}

#[test]
fn usb_detail_is_configurable_unless_another_program_talks() {
    let (_, snap) = golden_snapshot();
    let kb = to_detail(&snap, true, false).keyboard.unwrap();
    assert_eq!(kb.connection_path, ConnectionPath::Usb);
    assert!(kb.configurable);
    assert_eq!((kb.cached_at_ms, kb.receiver_firmware), (None, None));
    assert!(!to_detail(&snap, true, true).keyboard.unwrap().configurable);
    assert!(
        !to_detail(&snap, false, false)
            .keyboard
            .unwrap()
            .configurable
    );
}

#[test]
fn receiver_detail_shows_the_cache_or_an_empty_view() {
    let (_, snap) = golden_snapshot();
    let cache = cache_of(&snap, 1_234);
    let detail = to_receiver_detail(Some(&cache), Some("d.3.1".to_owned()), true, false);
    assert_eq!(detail.transport, "2.4 GHz (Keychron Link)");
    assert_eq!(detail.protocol, "VIA 12 / Keychron HE 4");
    assert_eq!(detail.summary.connection, Connection::Connected);
    let kb = detail.keyboard.unwrap();
    assert_eq!(kb.connection_path, ConnectionPath::Receiver);
    assert!(!kb.configurable);
    assert_eq!(kb.cached_at_ms, Some(1_234));
    assert_eq!(kb.receiver_firmware.as_deref(), Some("d.3.1"));
    assert!(kb.he.is_some() && kb.keymap.is_some());

    let empty = to_receiver_detail(None, None, true, false);
    assert_eq!(empty.protocol, "Keychron Link");
    let kb = empty.keyboard.unwrap();
    assert!(kb.he.is_none() && kb.lighting.is_none() && kb.timers.is_none() && kb.keymap.is_none());
    assert_eq!(kb.cached_at_ms, None);

    let gone = to_receiver_detail(Some(&cache), None, false, true);
    assert_eq!(gone.summary.connection, Connection::Disconnected);
    assert!(!gone.keyboard.unwrap().other_configurator);
}

#[test]
fn cache_round_trips_through_a_file_without_leftovers() {
    let (_, snap) = golden_snapshot();
    let cache = cache_of(&snap, 99);
    let dir = std::env::temp_dir().join(format!("myhub-k2he-cache-{}", std::process::id()));
    let path = cache_path(&dir);
    assert_eq!(KeyboardCache::load(&path), None);
    cache.save(&path).unwrap();
    cache.save(&path).unwrap();
    assert_eq!(KeyboardCache::load(&path), Some(cache));
    let entries = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
    assert_eq!(entries, 1);
    std::fs::write(&path, b"{ not json").unwrap();
    assert_eq!(KeyboardCache::load(&path), None);
    let _ = std::fs::remove_dir_all(&dir);
}
