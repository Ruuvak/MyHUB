//! Backups of the onboard profile sector taken before a flash write.
//!
//! `<root>/<deviceId>/<UTC yyyyMMddTHHmmssZ>_sector_<hex4>.json` holding
//! `{deviceId, sector, hex, crc, takenAt}`. The file is created new (an existing
//! file is never overwritten), synced to disk, and only then does the caller
//! write anything to the mouse. The newest [`KEEP_BACKUPS`] files per device are
//! kept, plus the oldest backup of each sector forever (the first image MyHUB
//! ever saw of it).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Files kept per device.
pub const KEEP_BACKUPS: usize = 50;

/// A UTC time as the file name and the JSON field show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtcStamp {
    /// `20261015T093005Z`, sorts like time.
    pub compact: String,
    /// `2026-10-15T09:30:05Z`.
    pub iso: String,
}

/// Formats Unix seconds as UTC (proleptic Gregorian, civil-from-days).
pub fn utc_stamp(unix_secs: u64) -> UtcStamp {
    let days = i64::try_from(unix_secs / 86_400).unwrap_or(0);
    let secs = unix_secs % 86_400;
    let (hour, minute, second) = (secs / 3600, secs % 3600 / 60, secs % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    UtcStamp {
        compact: format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"),
        iso: format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
    }
}

fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Writes the backup file and returns its path. Any failure is an error text
/// (the caller must not write to the mouse then). `bytes` is the whole sector;
/// the CRC shown is the one stored in its last two bytes.
pub fn write_backup(
    root: &Path,
    device_id: &str,
    taken_at_unix: u64,
    sector: u16,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    if !plain_id(device_id) {
        return Err(format!("device id {device_id:?} is not a plain name"));
    }
    let dir = root.join(device_id);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let stamp = utc_stamp(taken_at_unix);
    let crc = match bytes {
        [.., hi, lo] => u16::from_be_bytes([*hi, *lo]),
        _ => 0,
    };
    let json = serde_json::json!({
        "deviceId": device_id,
        "sector": sector,
        "hex": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "crc": format!("0x{crc:04x}"),
        "takenAt": stamp.iso,
    });
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;

    // Two backups in one second get a numeric suffix instead of failing.
    for attempt in 0..10u32 {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("_{attempt}")
        };
        let path = dir.join(format!(
            "{}_sector_{sector:04x}{suffix}.json",
            stamp.compact
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(text.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                prune(&dir, KEEP_BACKUPS);
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("cannot create {}: {e}", path.display())),
        }
    }
    Err(format!(
        "too many backups in the same second in {}",
        dir.display()
    ))
}

/// The sector number in a backup file name `<stamp>_sector_<hex4>[_n].json`.
fn sector_of(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let rest = name.split_once("_sector_")?.1;
    Some(rest.get(0..4)?.to_owned())
}

/// Deletes the `.json` files of `dir` except the newest `keep` and the oldest
/// file of each sector (names start with the UTC stamp, so name order is time
/// order). Failures are logged, not fatal.
pub fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    names.sort();
    let mut oldest_of = std::collections::HashSet::new();
    let mut protected = std::collections::HashSet::new();
    for (i, path) in names.iter().enumerate() {
        if let Some(sector) = sector_of(path)
            && oldest_of.insert(sector)
        {
            protected.insert(i);
        }
    }
    let excess = names.len().saturating_sub(keep);
    for (i, old) in names.into_iter().enumerate().take(excess) {
        if protected.contains(&i) {
            continue;
        }
        if let Err(e) = fs::remove_file(&old) {
            log_line!("myhub: cannot delete old backup {}: {e}", old.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "myhub-backup-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn utc_stamps_are_right() {
        assert_eq!(utc_stamp(0).compact, "19700101T000000Z");
        assert_eq!(utc_stamp(951_782_400).iso, "2000-02-29T00:00:00Z");
        assert_eq!(utc_stamp(1_760_520_605).iso, "2025-10-15T09:30:05Z");
        assert_eq!(utc_stamp(1_760_520_605).compact, "20251015T093005Z");
        assert_eq!(utc_stamp(4_102_444_799).iso, "2099-12-31T23:59:59Z");
    }

    #[test]
    fn a_backup_holds_the_documented_fields() {
        let root = temp_dir();
        let mut sector = vec![0x11u8; 255];
        sector[253] = 0xa4;
        sector[254] = 0x98;
        let path = write_backup(&root, "dev-1", 1_760_520_605, 3, &sector).unwrap();
        assert!(
            path.ends_with("20251015T093005Z_sector_0003.json"),
            "{path:?}"
        );
        assert_eq!(path.parent().unwrap(), root.join("dev-1"));
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["deviceId"], "dev-1");
        assert_eq!(value["sector"], 3);
        assert_eq!(value["crc"], "0xa498");
        assert_eq!(value["takenAt"], "2025-10-15T09:30:05Z");
        assert_eq!(value["hex"].as_str().unwrap().len(), 510);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_backup_in_the_same_second_does_not_overwrite() {
        let root = temp_dir();
        let a = write_backup(&root, "dev-1", 100, 3, &[1; 255]).unwrap();
        let b = write_backup(&root, "dev-1", 100, 3, &[2; 255]).unwrap();
        assert_ne!(a, b);
        assert!(fs::read_to_string(&a).unwrap().contains("0101"));
        assert!(fs::read_to_string(&b).unwrap().contains("0202"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_newest_fifty_and_the_oldest_of_each_sector_are_kept() {
        let root = temp_dir();
        let name = |t: u64, sector: u16| {
            root.join("dev-1")
                .join(format!("{}_sector_{sector:04x}.json", utc_stamp(t).compact))
        };
        // The very first backups of sectors 3 and 4, then 60 more of sector 3.
        write_backup(&root, "dev-1", 1000, 3, &[0; 255]).unwrap();
        write_backup(&root, "dev-1", 1001, 4, &[0; 255]).unwrap();
        let mut last = None;
        for t in 0..60u64 {
            last = Some(write_backup(&root, "dev-1", 2000 + t, 3, &[0; 255]).unwrap());
        }
        let count = fs::read_dir(root.join("dev-1")).unwrap().count();
        // Newest 50, plus the oldest of sector 3 and of sector 4 (outside them).
        assert_eq!(count, KEEP_BACKUPS + 2);
        assert!(last.unwrap().exists());
        assert!(name(1000, 3).exists(), "oldest of sector 3");
        assert!(name(1001, 4).exists(), "oldest of sector 4");
        // A middle one (older than the newest 50) is gone.
        assert!(!name(2005, 3).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bad_ids_and_unwritable_roots_are_errors() {
        let root = temp_dir();
        for bad in ["", "..", "a/b", r"a\b", "a.b"] {
            assert!(write_backup(&root, bad, 0, 3, &[0; 255]).is_err(), "{bad}");
        }
        // The root is a file: the directory cannot be created.
        let file = root.join("not-a-dir");
        fs::write(&file, "x").unwrap();
        assert!(write_backup(&file, "dev-1", 0, 3, &[0; 255]).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
