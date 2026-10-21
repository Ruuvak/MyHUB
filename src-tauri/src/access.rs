//! Device access problems on Linux. When a matching HID device is found but opening it
//! fails with a permission error, the worker records the module here; the UI shows a hint
//! with the udev rule. An open that succeeds clears the module. On Windows nothing is ever
//! recorded, so the list stays empty.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::device_manager::lock;

pub const ACCESS_ISSUES_EVENT: &str = "access-issues-changed";

/// The udev rule shipped in the .deb, also offered as a file to save.
pub const UDEV_RULE: &str = include_str!("../linux/70-myhub.rules");
const RULE_FILE: &str = "70-myhub.rules";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccessIssue {
    pub module: String,
}

/// The modules whose device could not be opened for lack of permission.
#[derive(Default)]
pub struct AccessIssues(Mutex<BTreeSet<&'static str>>);

fn to_list(set: &BTreeSet<&'static str>) -> Vec<AccessIssue> {
    set.iter()
        .map(|module| AccessIssue {
            module: (*module).to_owned(),
        })
        .collect()
}

impl AccessIssues {
    /// Sets or clears the module. When the set changed, `on_change` gets the new list
    /// while the lock is still held, so events leave in the order of the changes.
    fn update(&self, module: &'static str, denied: bool, on_change: impl FnOnce(Vec<AccessIssue>)) {
        let mut set = lock(&self.0);
        let changed = if denied {
            set.insert(module)
        } else {
            set.remove(module)
        };
        if changed {
            on_change(to_list(&set));
        }
    }

    fn list(&self) -> Vec<AccessIssue> {
        to_list(&lock(&self.0))
    }
}

/// `true` when a hidapi open error is a missing permission (EACCES). The Linux backend
/// words it "failed to open device with path ...: Permission denied (os error 13)".
pub fn is_permission_error(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("permission denied") || text.contains("os error 13")
}

/// Records the result of an open for `module` and tells the UI when the list changed.
/// `error` is the open error text, `None` for a successful open (or a device that is gone
/// or switched off).
pub fn report_open(app: &AppHandle, module: &'static str, error: Option<&str>) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let Some(issues) = app.try_state::<AccessIssues>() else {
        return;
    };
    let denied = error.is_some_and(is_permission_error);
    issues.update(module, denied, |list| {
        if let Err(e) = app.emit(ACCESS_ISSUES_EVENT, list) {
            eprintln!("myhub: cannot emit {ACCESS_ISSUES_EVENT}: {e}");
        }
    });
}

#[tauri::command]
pub fn get_access_issues(issues: State<'_, AccessIssues>) -> Vec<AccessIssue> {
    issues.list()
}

/// Writes the bundled udev rule to the Downloads folder (the home folder when there is
/// none) and returns the file path.
#[tauri::command]
pub fn save_udev_rule(app: AppHandle) -> Result<String, String> {
    let dir = match app.path().download_dir() {
        Ok(dir) => dir,
        Err(_) => app.path().home_dir().map_err(|e| e.to_string())?,
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path: PathBuf = dir.join(RULE_FILE);
    std::fs::write(&path, UDEV_RULE).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(issues: &AccessIssues, module: &'static str, denied: bool) -> bool {
        let mut hit = false;
        issues.update(module, denied, |_| hit = true);
        hit
    }

    #[test]
    fn permission_errors_are_recognised() {
        assert!(is_permission_error(
            "hid error: hidapi error: failed to open device with path /dev/hidraw3: Permission denied (os error 13)"
        ));
        assert!(is_permission_error("open failed (os error 13)"));
        assert!(!is_permission_error("hidapi error: device not found"));
        assert!(!is_permission_error("no such file (os error 2)"));
    }

    #[test]
    fn issues_are_set_and_cleared_per_module() {
        let issues = AccessIssues::default();
        assert!(issues.list().is_empty());
        assert!(
            changed(&issues, "logitech", true),
            "first set changes the list"
        );
        assert!(
            !changed(&issues, "logitech", true),
            "repeat changes nothing"
        );
        assert!(changed(&issues, "keychron", true));
        let modules: Vec<_> = issues.list().into_iter().map(|i| i.module).collect();
        assert_eq!(modules, ["keychron", "logitech"]);
        assert!(changed(&issues, "logitech", false));
        assert!(!changed(&issues, "logitech", false));
        assert_eq!(issues.list().len(), 1);
    }

    #[test]
    fn rule_covers_both_vendors() {
        assert!(UDEV_RULE.contains("046d") && UDEV_RULE.contains("c547"));
        assert!(UDEV_RULE.contains("3434"));
        assert!(UDEV_RULE.contains("uaccess"));
    }

    #[test]
    fn deb_config_ships_the_rule_and_scripts() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let deb = &conf["bundle"]["linux"]["deb"];
        let source = deb["files"]["/usr/lib/udev/rules.d/70-myhub.rules"]
            .as_str()
            .expect("rule entry");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(root.join(source).is_file());
        for key in ["postInstallScript", "postRemoveScript"] {
            let script = deb[key].as_str().expect(key);
            assert!(root.join(script).is_file(), "{key}");
        }
    }
}
