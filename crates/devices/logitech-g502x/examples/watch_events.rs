//! Read-only event watcher against real hardware.
//!
//! Usage: `watch_events [--seconds <n>]` (default 30)
//!
//! Discovers the device, then once per second reads the battery (0x1004 fn1,
//! read tier) and prints every notification the session collected, including
//! HID++ 1.0 0x41 connection messages. After a 0x41 the session refuses
//! requests until `discover()` runs again, which this tool retries each second.
//! There are no write-capable options. Serial numbers and device paths are
//! never printed.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use hidpp::FeatureId;
use logitech_g502x::DEVICE_INDEX;
use myhub_transport::{HidApi, HidppSession, SessionError, find_receivers};

fn main() -> ExitCode {
    let mut seconds = 30u64;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match (arg.as_str(), it.next().and_then(|v| v.parse().ok())) {
            ("--seconds", Some(n)) => seconds = n,
            _ => {
                eprintln!("usage: watch_events [--seconds <n>]");
                return ExitCode::from(2);
            }
        }
    }

    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("could not initialise hidapi: {e}");
            return ExitCode::from(3);
        }
    };
    let Some(receiver) = find_receivers(&api).into_iter().next() else {
        eprintln!("no receiver HID++ interface found");
        return ExitCode::from(1);
    };
    let mut session = match HidppSession::open(&api, &receiver) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("could not open receiver: {e}");
            return ExitCode::from(3);
        }
    };
    session.set_device_index(DEVICE_INDEX);
    println!(
        "short reports available: {}",
        session.short_reports_available()
    );

    let start = Instant::now();
    let mut discovered = false;
    while start.elapsed() < Duration::from_secs(seconds) {
        let t = start.elapsed().as_secs_f32();
        if !discovered || session.needs_rediscovery() {
            match session.discover() {
                Ok(()) => {
                    discovered = true;
                    println!("{t:6.1}s discover ok, {} features", session.table().len());
                }
                Err(e) => println!("{t:6.1}s discover failed: {e}"),
            }
        }
        if discovered && !session.needs_rediscovery() {
            match session.request_feature(FeatureId::UNIFIED_BATTERY, 1, [0; 16]) {
                Ok(p) => println!("{t:6.1}s battery {} % raw {:02x?}", p[0], &p[..4]),
                Err(SessionError::NeedsRediscovery) => {
                    println!("{t:6.1}s needs rediscovery (0x41 seen)")
                }
                Err(e) => println!("{t:6.1}s battery read failed: {e}"),
            }
        }
        for message in session.drain_notifications() {
            println!("{t:6.1}s notification {message:?}");
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    ExitCode::SUCCESS
}
