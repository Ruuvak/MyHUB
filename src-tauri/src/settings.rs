//! Persisted desktop settings (JSON in the app config dir).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use myhub_core::DevicePrefs;
use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "settings.json";
const BACKUP_FILE: &str = "settings.json.bak";
#[cfg(test)]
const TEMP_FILE: &str = "settings.json.tmp";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Auto,
    Pl,
    En,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub language: Language,
    pub close_to_tray: bool,
    pub start_minimized: bool,
    /// Master switch for volatile device writes (DPI, report rate). Off by default.
    pub allow_device_writes: bool,
    /// After app start and after each reconnect, go back to the last mode and
    /// DPI and rate the user chose (app control mode). On by default; needs
    /// `allowDeviceWrites` to do anything.
    pub restore_last_mode: bool,
    /// Start MyHUB at login (registry Run key on Windows, XDG autostart on Linux). On by default.
    pub launch_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            language: Language::Auto,
            close_to_tray: true,
            start_minimized: false,
            allow_device_writes: false,
            restore_last_mode: true,
            launch_at_login: true,
        }
    }
}

impl Settings {
    /// Checks a value coming from the UI. The enums already reject unknown
    /// variants during deserialization, so nothing else can be invalid today.
    pub fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// Parses a settings file field by field: a missing field or a value of
    /// the wrong type or an unknown variant falls back to its default.
    /// Fails only when the text is not a JSON object.
    pub fn from_json_lenient(text: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let serde_json::Value::Object(map) = value else {
            return Err("settings file is not a JSON object".to_string());
        };
        let defaults = Self::default();
        fn field<T: serde::de::DeserializeOwned>(
            map: &serde_json::Map<String, serde_json::Value>,
            key: &str,
            default: T,
        ) -> T {
            map.get(key)
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or(default)
        }
        Ok(Self {
            theme: field(&map, "theme", defaults.theme),
            language: field(&map, "language", defaults.language),
            close_to_tray: field(&map, "closeToTray", defaults.close_to_tray),
            start_minimized: field(&map, "startMinimized", defaults.start_minimized),
            allow_device_writes: field(&map, "allowDeviceWrites", defaults.allow_device_writes),
            // The old `restoreOnConnect` is ignored.
            restore_last_mode: field(&map, "restoreLastMode", defaults.restore_last_mode),
            launch_at_login: field(&map, "launchAtLogin", defaults.launch_at_login),
        })
    }
}

/// Resolves the `auto` language from an OS locale tag such as `pl-PL`.
/// Returns `Language::Pl` or `Language::En`, never `Auto`.
pub fn resolve_language(language: Language, os_locale: Option<&str>) -> Language {
    match language {
        Language::Auto => match os_locale {
            Some(tag) if tag.to_ascii_lowercase().starts_with("pl") => Language::Pl,
            _ => Language::En,
        },
        explicit => explicit,
    }
}

/// Loads settings from `path`. A missing file gives defaults. A corrupt file
/// is renamed to `settings.json.bak` next to it and defaults are used.
pub fn load_from(path: &Path) -> Settings {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Settings::default(),
        Err(e) => {
            eprintln!("myhub: cannot read {}: {e}; using defaults", path.display());
            return Settings::default();
        }
    };
    match Settings::from_json_lenient(&text) {
        Ok(settings) => settings,
        Err(reason) => {
            let backup = path.with_file_name(BACKUP_FILE);
            match fs::rename(path, &backup) {
                Ok(()) => eprintln!(
                    "myhub: corrupt settings ({reason}); moved to {} and using defaults",
                    backup.display()
                ),
                Err(e) => eprintln!(
                    "myhub: corrupt settings ({reason}); backup to {} failed: {e}; using defaults",
                    backup.display()
                ),
            }
            Settings::default()
        }
    }
}

/// Writes settings atomically: temp file in the same directory, then rename.
pub fn save_to(path: &Path, settings: &Settings) -> io::Result<()> {
    let json = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    write_atomic(path, &json)
}

fn write_atomic(path: &Path, json: &str) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let mut temp_name = path.file_name().unwrap_or_default().to_os_string();
    temp_name.push(".tmp");
    let temp = path.with_file_name(temp_name);
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Directory of the per-device prefs files inside the app config dir.
pub const DEVICES_DIR: &str = "devices";

/// Device ids the manager knows. Only these get a prefs file.
pub const KNOWN_DEVICE_IDS: &[&str] = &[logitech_g502x::DEVICE_ID, sony_wf_c710n::DEVICE_ID];

/// Path of the prefs file of one device: `<config_dir>/devices/<id>.json`.
/// `None` for an id the manager does not know, or that is not a plain
/// `[A-Za-z0-9_-]` name, so an id from the UI can never point outside the
/// devices directory.
pub fn prefs_path(config_dir: &Path, id: &str) -> Option<PathBuf> {
    let plain = KNOWN_DEVICE_IDS.contains(&id)
        && !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| config_dir.join(DEVICES_DIR).join(format!("{id}.json")))
}

/// Loads device prefs. A missing file is `None`; a corrupt or unreadable file
/// is logged and ignored (`None`), and left in place.
pub fn load_prefs(path: &Path) -> Option<DevicePrefs> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("myhub: cannot read {}: {e}; ignoring", path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(prefs) => Some(prefs),
        Err(e) => {
            eprintln!(
                "myhub: corrupt device prefs {}: {e}; ignoring",
                path.display()
            );
            None
        }
    }
}

/// Writes device prefs atomically (temp file in the same directory, then rename).
pub fn save_prefs(path: &Path, prefs: &DevicePrefs) -> io::Result<()> {
    let json = serde_json::to_string_pretty(prefs).map_err(io::Error::other)?;
    write_atomic(path, &json)
}

/// Managed state: current settings plus where they live on disk.
pub struct SettingsState {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsState {
    pub fn load(path: PathBuf) -> Self {
        let current = Mutex::new(load_from(&path));
        Self { path, current }
    }

    fn lock(&self) -> MutexGuard<'_, Settings> {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn get(&self) -> Settings {
        self.lock().clone()
    }

    /// Validates, saves to disk, then stores in memory. On error nothing changes.
    pub fn set(&self, settings: Settings) -> Result<Settings, String> {
        settings.validate()?;
        let mut guard = self.lock();
        save_to(&self.path, &settings)
            .map_err(|e| format!("cannot save {}: {e}", self.path.display()))?;
        *guard = settings.clone();
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "myhub-settings-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join(SETTINGS_FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn defaults_are_as_specified() {
        let d = Settings::default();
        assert_eq!(d.theme, Theme::System);
        assert_eq!(d.language, Language::Auto);
        assert!(d.close_to_tray);
        assert!(!d.start_minimized);
        assert!(!d.allow_device_writes);
        assert!(d.restore_last_mode);
        assert!(d.launch_at_login);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = TempDir::new();
        assert_eq!(load_from(&dir.file()), Settings::default());
    }

    #[test]
    fn partial_file_fills_defaults_and_ignores_unknown_fields() {
        let dir = TempDir::new();
        fs::write(
            dir.file(),
            r#"{"theme":"dark","startMinimized":true,"future":42}"#,
        )
        .unwrap();
        let s = load_from(&dir.file());
        assert_eq!(s.theme, Theme::Dark);
        assert!(s.start_minimized);
        assert_eq!(s.language, Language::Auto);
        assert!(s.close_to_tray);
    }

    #[test]
    fn bad_field_value_falls_back_without_backup() {
        let dir = TempDir::new();
        fs::write(dir.file(), r#"{"theme":"purple","closeToTray":false}"#).unwrap();
        let s = load_from(&dir.file());
        assert_eq!(s.theme, Theme::System);
        assert!(!s.close_to_tray);
        assert!(dir.file().exists());
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaults_returned() {
        let dir = TempDir::new();
        fs::write(dir.file(), "{ not json").unwrap();
        assert_eq!(load_from(&dir.file()), Settings::default());
        assert!(!dir.file().exists());
        let backup = dir.0.join(BACKUP_FILE);
        assert_eq!(fs::read_to_string(backup).unwrap(), "{ not json");
    }

    #[test]
    fn non_object_json_counts_as_corrupt() {
        let dir = TempDir::new();
        fs::write(dir.file(), "[1,2]").unwrap();
        assert_eq!(load_from(&dir.file()), Settings::default());
        assert!(dir.0.join(BACKUP_FILE).exists());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new();
        let wanted = Settings {
            theme: Theme::Light,
            language: Language::Pl,
            close_to_tray: false,
            start_minimized: true,
            allow_device_writes: true,
            restore_last_mode: false,
            launch_at_login: false,
        };
        save_to(&dir.file(), &wanted).unwrap();
        assert!(!dir.0.join(TEMP_FILE).exists());
        assert_eq!(load_from(&dir.file()), wanted);
        // Overwrite an existing file.
        save_to(&dir.file(), &Settings::default()).unwrap();
        assert_eq!(load_from(&dir.file()), Settings::default());
    }

    #[test]
    fn save_creates_missing_directory() {
        let dir = TempDir::new();
        let nested = dir.0.join("a").join("b").join(SETTINGS_FILE);
        save_to(&nested, &Settings::default()).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn json_uses_camel_case_and_lowercase_enums() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert_eq!(
            json,
            r#"{"theme":"system","language":"auto","closeToTray":true,"startMinimized":false,"allowDeviceWrites":false,"restoreLastMode":true,"launchAtLogin":true}"#
        );
    }

    #[test]
    fn state_set_persists_and_returns_stored_value() {
        let dir = TempDir::new();
        let state = SettingsState::load(dir.file());
        assert_eq!(state.get(), Settings::default());
        let wanted = Settings {
            theme: Theme::Dark,
            ..Settings::default()
        };
        assert_eq!(state.set(wanted.clone()).unwrap(), wanted);
        assert_eq!(state.get(), wanted);
        assert_eq!(SettingsState::load(dir.file()).get(), wanted);
    }

    #[test]
    fn old_settings_file_without_the_new_fields_still_loads() {
        let dir = TempDir::new();
        fs::write(dir.file(), r#"{"theme":"dark","closeToTray":false}"#).unwrap();
        let s = load_from(&dir.file());
        assert!(!s.allow_device_writes);
        assert!(s.restore_last_mode, "the new switch is on by default");
        let t = Settings::from_json_lenient(r#"{"allowDeviceWrites":true}"#).unwrap();
        assert!(t.allow_device_writes);
        assert!(t.restore_last_mode);
    }

    #[test]
    fn an_old_file_with_restore_on_connect_loads_and_the_field_is_ignored() {
        for old in [false, true] {
            let text =
                format!(r#"{{"theme":"dark","allowDeviceWrites":true,"restoreOnConnect":{old}}}"#);
            let s = Settings::from_json_lenient(&text).unwrap();
            assert!(
                s.restore_last_mode,
                "restoreOnConnect={old} must not decide"
            );
            assert!(s.allow_device_writes);
            assert_eq!(s.theme, Theme::Dark);
        }
        let t = Settings::from_json_lenient(r#"{"restoreLastMode":false}"#).unwrap();
        assert!(!t.restore_last_mode);
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!json.contains("restoreOnConnect"));
    }

    #[test]
    fn old_file_without_launch_at_login_defaults_to_true() {
        let dir = TempDir::new();
        fs::write(dir.file(), r#"{"theme":"dark","startMinimized":true}"#).unwrap();
        assert!(load_from(&dir.file()).launch_at_login);
        let s = Settings::from_json_lenient(r#"{"launchAtLogin":false}"#).unwrap();
        assert!(!s.launch_at_login);
        let strict: Settings = serde_json::from_str(r#"{"closeToTray":false}"#).unwrap();
        assert!(strict.launch_at_login);
        assert!(!strict.close_to_tray);
    }

    #[test]
    fn prefs_path_rejects_ids_that_could_escape() {
        let base = Path::new("cfg");
        assert_eq!(
            prefs_path(base, "logitech-g502x"),
            Some(base.join("devices").join("logitech-g502x.json"))
        );
        assert_eq!(prefs_path(base, "unknown-device"), None);
        for bad in [
            "",
            "..",
            "a/b",
            r"a\b",
            "../x",
            "a.b",
            "a b",
            &"x".repeat(65),
        ] {
            assert_eq!(prefs_path(base, bad), None, "{bad}");
        }
    }

    #[test]
    fn prefs_round_trip_missing_and_corrupt() {
        let dir = TempDir::new();
        let path = prefs_path(&dir.0, "logitech-g502x").unwrap();
        assert_eq!(load_prefs(&path), None);
        let prefs = DevicePrefs {
            dpi: Some(1200),
            report_rate_hz: Some(500),
            mode: myhub_core::OnboardMode::Host,
            profile: Some(3),
        };
        save_prefs(&path, &prefs).unwrap();
        assert!(!path.with_file_name("logitech-g502x.json.tmp").exists());
        assert_eq!(load_prefs(&path), Some(prefs));
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_prefs(&path), None);
        assert!(path.exists());
    }

    #[test]
    fn language_auto_resolves_from_locale() {
        assert_eq!(
            resolve_language(Language::Auto, Some("pl-PL")),
            Language::Pl
        );
        assert_eq!(resolve_language(Language::Auto, Some("PL")), Language::Pl);
        assert_eq!(
            resolve_language(Language::Auto, Some("en-US")),
            Language::En
        );
        assert_eq!(resolve_language(Language::Auto, None), Language::En);
        assert_eq!(resolve_language(Language::Pl, Some("en-US")), Language::Pl);
        assert_eq!(resolve_language(Language::En, Some("pl-PL")), Language::En);
    }
}
