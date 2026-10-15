mod common;

use common::{fixture_bytes, hex};
use hidpp::battery::{self, BatteryStatus};
use hidpp::dpi::{self, DpiEntry, DpiList};
use hidpp::error::{EncodeError, ParseError};
use hidpp::report_rate::{self, ReportRateList};

// ---- 0x2201 ----

#[test]
fn sensor_count() {
    assert_eq!(
        dpi::parse_sensor_count(&fixture_bytes("2201_fn0_sensor_count")).unwrap(),
        1
    );
    assert!(dpi::parse_sensor_count(&[]).is_err());
}

#[test]
fn dpi_list_fixture_is_100_to_25600_step_50() {
    let raw = fixture_bytes("2201_fn1_dpi_list_s0");
    assert_eq!(raw[..7], hex("000064e0326400")[..]);
    let list = DpiList::parse(&raw).unwrap();
    assert_eq!(list.sensor, 0);
    assert_eq!(
        list.entries,
        vec![DpiEntry::Range {
            min: 100,
            max: 25600,
            step: 50
        }]
    );
    for ok in [100u16, 150, 1600, 25600, 800] {
        assert!(list.contains(ok), "{ok}");
    }
    for bad in [0u16, 99, 101, 125, 25650, 25601, 65535] {
        assert!(!list.contains(bad), "{bad}");
    }
}

#[test]
fn dpi_list_discrete_values_and_malformed() {
    let list = DpiList::parse(&hex("0003200640000000")).unwrap();
    assert_eq!(
        list.entries,
        vec![DpiEntry::Discrete(800), DpiEntry::Discrete(1600),]
    );
    assert!(list.contains(800) && !list.contains(801));

    // Mixed: discrete 400, then a range 800..=1600 step 100.
    let mixed = DpiList::parse(&hex("0001900320e06406400000")).unwrap();
    assert_eq!(mixed.sensor, 0);
    assert_eq!(
        mixed.entries,
        vec![
            DpiEntry::Discrete(400),
            DpiEntry::Range {
                min: 800,
                max: 1600,
                step: 100
            }
        ]
    );
    assert!(
        mixed.contains(400)
            && mixed.contains(1500)
            && !mixed.contains(1550)
            && !mixed.contains(500)
    );
}

#[test]
fn dpi_list_errors() {
    // Step marker with nothing before it.
    assert!(matches!(
        DpiList::parse(&hex("00e032640000")),
        Err(ParseError::MalformedDpiList(_))
    ));
    // Step marker with no end value.
    assert!(matches!(
        DpiList::parse(&hex("000064e03200")),
        Err(ParseError::MalformedDpiList(_))
    ));
    // Step 0.
    assert!(matches!(
        DpiList::parse(&hex("000064e0006400")),
        Err(ParseError::MalformedDpiList(_))
    ));
    // End below start.
    assert!(matches!(
        DpiList::parse(&hex("006400e0320064")),
        Err(ParseError::MalformedDpiList(_))
    ));
    assert!(matches!(
        DpiList::parse(&[0, 0]),
        Err(ParseError::TooShort { .. })
    ));
}

#[test]
fn current_dpi_fixture() {
    let s = dpi::parse_sensor_dpi(&fixture_bytes("2201_fn2_dpi_s0")).unwrap();
    assert_eq!((s.sensor, s.current, s.default), (0, 1600, 1600));
    assert!(dpi::parse_sensor_dpi(&[0, 1, 2, 3]).is_err());
}

#[test]
fn set_sensor_dpi_encoder() {
    let list = DpiList::parse(&fixture_bytes("2201_fn1_dpi_list_s0")).unwrap();

    // Version 2 needs an explicit level; None is refused.
    assert_eq!(
        dpi::set_sensor_dpi_params(0, 1600, None, 2, &list),
        Err(EncodeError::DpiLevelRequired { version: 2 })
    );
    let p = dpi::set_sensor_dpi_params(0, 1600, Some(0), 2, &list).unwrap();
    assert_eq!(p[..4], [0x00, 0x06, 0x40, 0x00]);
    assert!(p[4..].iter().all(|&b| b == 0));

    // Version 2 with level.
    let p = dpi::set_sensor_dpi_params(0, 25600, Some(3), 2, &list).unwrap();
    assert_eq!(p[..4], [0x00, 0x64, 0x00, 0x03]);

    // Version 1 has no level byte.
    let p = dpi::set_sensor_dpi_params(0, 800, None, 1, &list).unwrap();
    assert_eq!(p[..4], [0x00, 0x03, 0x20, 0x00]);
    assert_eq!(
        dpi::set_sensor_dpi_params(0, 800, Some(1), 1, &list),
        Err(EncodeError::DpiLevelNeedsV2 { version: 1 })
    );

    // Off step, out of range, zero.
    for bad in [0u16, 99, 125, 25650, 40000] {
        assert_eq!(
            dpi::set_sensor_dpi_params(0, bad, Some(1), 2, &list),
            Err(EncodeError::DpiNotSupported { dpi: bad })
        );
    }
    assert_eq!(
        dpi::set_sensor_dpi_params(1, 800, Some(1), 2, &list),
        Err(EncodeError::SensorMismatch {
            sensor: 1,
            list_sensor: 0
        })
    );
    assert_eq!(dpi::sensor_params(1)[0], 1);
}

// ---- 0x8060 ----

#[test]
fn report_rate_list_fixture() {
    let list = ReportRateList::parse(&fixture_bytes("8060_fn0_rate_list")).unwrap();
    assert_eq!(list.bitmask, 0x8B);
    assert_eq!(list.intervals_ms(), vec![1, 2, 4, 8]);
    assert_eq!(list.rates_hz(), vec![1000, 500, 250, 125]);
    for ms in [1u8, 2, 4, 8] {
        assert!(list.supports(ms));
    }
    for ms in [0u8, 3, 5, 6, 7, 9, 255] {
        assert!(!list.supports(ms));
    }
    assert!(ReportRateList::parse(&[]).is_err());
}

#[test]
fn report_rate_current_fixture() {
    let ms = report_rate::parse_current_interval_ms(&fixture_bytes("8060_fn1_rate")).unwrap();
    assert_eq!(ms, 1);
    assert_eq!(report_rate::interval_to_hz(ms), 1000);
    assert_eq!(report_rate::interval_to_hz(8), 125);
    assert!(report_rate::parse_current_interval_ms(&[0]).is_err());
    assert!(report_rate::parse_current_interval_ms(&[9]).is_err());
    assert!(report_rate::parse_current_interval_ms(&[]).is_err());
}

#[test]
fn set_report_rate_encoder() {
    let list = ReportRateList::parse(&fixture_bytes("8060_fn0_rate_list")).unwrap();
    for ms in [1u8, 2, 4, 8] {
        let p = report_rate::set_report_rate_params(ms, &list).unwrap();
        assert_eq!(p[0], ms);
        assert!(p[1..].iter().all(|&b| b == 0));
    }
    for ms in [0u8, 3, 16] {
        assert_eq!(
            report_rate::set_report_rate_params(ms, &list),
            Err(EncodeError::RateNotSupported { interval_ms: ms })
        );
    }
}

// ---- 0x1004 ----

#[test]
fn battery_fixture() {
    let s = battery::parse_status(&fixture_bytes("1004_fn1_status")).unwrap();
    assert_eq!(
        s,
        BatteryStatus {
            percent: 55,
            level_flags: 0x04,
            charging_state: 0x00,
            external_power: 0x00,
        }
    );
    assert!(battery::parse_status(&[55, 4, 0]).is_err());
}

#[test]
fn battery_level_and_charging_names() {
    let s = battery::parse_status(&fixture_bytes("1004_fn1_status")).unwrap();
    assert_eq!(s.level_name(), Some("good"));
    assert_eq!(s.charging_state_name(), "discharging");
    for (b, name) in [(8u8, "full"), (4, "good"), (2, "low"), (1, "critical")] {
        assert_eq!(battery::level_name(b), Some(name));
    }
    for b in [0u8, 3, 5, 6, 9, 12, 0xFF] {
        assert_eq!(battery::level_name(b), None, "{b}");
    }
    let names = [
        "discharging",
        "recharging",
        "almost_full",
        "full",
        "slow_recharge",
        "invalid_battery",
        "thermal_error",
    ];
    for (n, name) in names.iter().enumerate() {
        assert_eq!(battery::charging_state_name(n as u8), *name);
    }
    assert_eq!(battery::charging_state_name(7), "unknown(7)");
    assert_eq!(battery::charging_state_name(0xFF), "unknown(255)");
}
