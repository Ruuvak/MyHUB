//! App updates: check, download and install, all in Rust.
//!
//! On Windows the plugin's `install` starts the NSIS installer and exits the process without a
//! `RunEvent::Exit`, so the device workers are stopped here before the installer starts.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::device_manager::DeviceManager;

const PROGRESS_EVENT: &str = "update-progress";

/// The update found by the last check, kept until it is installed.
#[derive(Default)]
pub struct PendingUpdate {
    update: Mutex<Option<Update>>,
    /// An install has started; checks and further installs are refused.
    installing: AtomicBool,
}

impl PendingUpdate {
    fn set(&self, update: Option<Update>) {
        *self
            .update
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = update;
    }

    fn take(&self) -> Option<Update> {
        self.update
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// Asks the update endpoint for a newer version. `null` when this is the latest.
/// Refused while an install runs.
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<Option<UpdateInfo>, String> {
    if pending.installing.load(Ordering::SeqCst) {
        return Err("update in progress".to_owned());
    }
    let updater = app.updater().map_err(|e| e.to_string())?;
    let found = updater.check().await.map_err(|e| e.to_string())?;
    let info = found.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        date: update.date.map(|d| d.to_string()),
    });
    // An install may have started during the check; it holds its own copy.
    if pending.installing.load(Ordering::SeqCst) {
        return Err("update in progress".to_owned());
    }
    pending.set(found);
    Ok(info)
}

/// Downloads the update found by `check_for_update`, returns the mouse to onboard mode
/// (device shutdown), installs it and restarts. Progress goes out as `update-progress`.
/// A started install consumes the pending update (a second call gets "no update pending");
/// a failed download puts it back so Install can be retried.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<(), String> {
    if pending.installing.swap(true, Ordering::SeqCst) {
        return Err("update in progress".to_owned());
    }
    let Some(update) = pending.take() else {
        pending.installing.store(false, Ordering::SeqCst);
        return Err("no update pending".to_owned());
    };
    let mut downloaded: u64 = 0;
    let downloaded_bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                let _ = app.emit(PROGRESS_EVENT, Progress { downloaded, total });
            },
            || {},
        )
        .await;
    let bytes = match downloaded_bytes {
        Ok(bytes) => bytes,
        Err(e) => {
            // Nothing was stopped yet.
            pending.set(Some(update));
            pending.installing.store(false, Ordering::SeqCst);
            return Err(e.to_string());
        }
    };
    // The installer may end the process without RunEvent::Exit (Windows). The shutdown waits
    // for running writes, so it must not block an async worker thread.
    let handle = app.clone();
    let stopped = tauri::async_runtime::spawn_blocking(move || {
        if let Some(devices) = handle.try_state::<DeviceManager>() {
            devices.shutdown();
        }
    })
    .await;
    if let Err(e) = stopped {
        pending.installing.store(false, Ordering::SeqCst);
        return Err(e.to_string());
    }
    if let Err(e) = update.install(bytes) {
        // The device workers are stopped now. Restarting the app brings them back; the
        // user sees the app reopen and can try again later.
        log_line!("myhub: update install failed: {e}");
        app.restart();
    }
    // Windows relaunches through the installer; elsewhere the app restarts itself.
    #[cfg(not(windows))]
    app.restart();
    #[cfg(windows)]
    Ok(())
}
