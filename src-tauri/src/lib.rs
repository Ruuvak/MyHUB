pub mod backup;
pub mod device_manager;
pub mod flyout;
pub mod settings;
pub mod tray;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use device_manager::{
    DeviceManager, REFRESH_WAIT, SAVE_WAIT, SaveRequest, SaveResult, WRITE_WAIT, WriteOp,
};
use myhub_core::{DeviceDetail, DevicePrefs, DeviceSummary};
use settings::{Settings, SettingsState};

const SETTINGS_CHANGED_EVENT: &str = "settings-changed";
const SELECT_DEVICE_EVENT: &str = "select-device";
/// Argument the OS autostart entry passes; such a launch always starts hidden in the tray.
const AUTOSTART_ARG: &str = "--autostart";

/// Brings the OS autostart entry in line with the setting. Failures are logged only.
/// Debug builds never touch it, so `tauri dev` does not register the dev exe.
fn sync_autostart(app: &AppHandle, launch_at_login: bool) {
    if cfg!(debug_assertions) {
        return;
    }
    let manager = app.autolaunch();
    let enabled = match manager.is_enabled() {
        Ok(enabled) => enabled,
        Err(e) => {
            eprintln!("myhub: cannot read autostart state: {e}");
            return;
        }
    };
    let result = match (launch_at_login, enabled) {
        (true, false) => manager.enable(),
        (false, true) => manager.disable(),
        _ => return,
    };
    if let Err(e) = result {
        eprintln!("myhub: cannot update autostart: {e}");
    }
}

#[derive(Serialize)]
struct AppInfo {
    name: &'static str,
    version: &'static str,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: "MyHUB",
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[tauri::command]
fn get_settings(state: State<'_, SettingsState>) -> Settings {
    state.get()
}

#[tauri::command]
fn set_settings(
    app: AppHandle,
    state: State<'_, SettingsState>,
    settings: Settings,
) -> Result<Settings, String> {
    let stored = state.set(settings)?;
    sync_autostart(&app, stored.launch_at_login);
    tray::apply_settings(&app, &stored);
    // The device status line is localized too.
    if let Some(devices) = app.try_state::<DeviceManager>() {
        devices.refresh_tray(&app);
        // readOnly depends on allowDeviceWrites.
        devices.settings_changed();
    }
    if let Err(e) = app.emit(SETTINGS_CHANGED_EVENT, &stored) {
        eprintln!("myhub: cannot emit {SETTINGS_CHANGED_EVENT}: {e}");
    }
    Ok(stored)
}

#[tauri::command]
fn list_devices(devices: State<'_, DeviceManager>) -> Vec<DeviceSummary> {
    devices.devices()
}

#[tauri::command]
fn get_device_detail(devices: State<'_, DeviceManager>, id: String) -> Option<DeviceDetail> {
    devices.detail(&id)
}

/// Re-reads the device now (read tier only). Waits up to five seconds for the
/// worker; `null` when the device is unknown, asleep, not readable or the
/// worker did not answer in time.
#[tauri::command]
async fn refresh_device(
    devices: State<'_, DeviceManager>,
    id: String,
) -> Result<Option<DeviceDetail>, String> {
    let Some(answer) = devices.request_refresh(&id) else {
        return Ok(None);
    };
    // The wait blocks, so it must not run on an async worker thread.
    tauri::async_runtime::spawn_blocking(move || answer.recv_timeout(REFRESH_WAIT).ok().flatten())
        .await
        .map_err(|e| e.to_string())
}

/// Runs a write command on the worker and waits for its answer. Errors are the
/// contract codes (`writesDisabled`, `disconnected`, `notHostMode`,
/// `outOfRange`, `verifyFailed`, `deviceError:<code>`).
async fn run_write(
    devices: State<'_, DeviceManager>,
    id: String,
    op: WriteOp,
) -> Result<DeviceDetail, String> {
    let answer = devices
        .request_write(&id, op)
        .ok_or_else(|| "disconnected".to_owned())?;
    // The wait blocks, so it must not run on an async worker thread.
    tauri::async_runtime::spawn_blocking(move || {
        answer
            .recv_timeout(WRITE_WAIT)
            .map_err(|_| "deviceError:timeout".to_owned())?
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Sets the current DPI (volatile, never flash). Needs `allowDeviceWrites`.
#[tauri::command]
async fn set_dpi(
    devices: State<'_, DeviceManager>,
    id: String,
    dpi: u16,
) -> Result<DeviceDetail, String> {
    run_write(devices, id, WriteOp::Dpi(dpi)).await
}

/// Sets the report rate in Hz (volatile, never flash). Needs `allowDeviceWrites`.
#[tauri::command]
async fn set_report_rate(
    devices: State<'_, DeviceManager>,
    id: String,
    hz: u16,
) -> Result<DeviceDetail, String> {
    run_write(devices, id, WriteOp::ReportRate(hz)).await
}

/// Saves DPI levels, indexes and report rate into the active onboard profile
/// (flash write). Needs `allowDeviceWrites`; the target is the active profile
/// only when `onboard.writableProfile` is set. A backup of the sector is taken
/// first. Errors: `writesDisabled`, `disconnected`, `notOnboardMode`,
/// `profileNotWritable`, `outOfRange`, `backupFailed`,
/// `flashWriteFailed:<restored|restoreFailed|notAttempted|notNeeded>`,
/// `deviceError:<code>`.
#[tauri::command]
async fn save_profile_to_mouse(
    devices: State<'_, DeviceManager>,
    id: String,
    dpi_levels: Option<Vec<u16>>,
    default_dpi_index: Option<u8>,
    shift_dpi_index: Option<u8>,
    report_rate_hz: Option<u16>,
) -> Result<SaveResult, String> {
    let request = SaveRequest {
        dpi_levels,
        default_dpi_index,
        shift_dpi_index,
        report_rate_hz,
    };
    let answer = devices
        .request_save(&id, request)
        .ok_or_else(|| "disconnected".to_owned())?;
    // The wait blocks, so it must not run on an async worker thread.
    tauri::async_runtime::spawn_blocking(move || {
        answer
            .recv_timeout(SAVE_WAIT)
            .map_err(|_| "deviceError:timeout".to_owned())?
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Enters the temporary HOST mode (0x8100 fn1, verified): the mouse buttons,
/// profiles and LEDs stop until it is left. Needs `allowDeviceWrites`. Errors:
/// `writesDisabled`, `disconnected`, `notHostMode`, `deviceError:<code>`.
#[tauri::command]
async fn enter_temporary_mode(
    devices: State<'_, DeviceManager>,
    id: String,
) -> Result<DeviceDetail, String> {
    run_mode(devices, id, true).await
}

/// Leaves the temporary HOST mode (back to the onboard profile). A no-op success
/// when this app did not switch the mouse.
#[tauri::command]
async fn leave_temporary_mode(
    devices: State<'_, DeviceManager>,
    id: String,
) -> Result<DeviceDetail, String> {
    run_mode(devices, id, false).await
}

async fn run_mode(
    devices: State<'_, DeviceManager>,
    id: String,
    enter: bool,
) -> Result<DeviceDetail, String> {
    let answer = devices
        .request_mode(&id, enter)
        .ok_or_else(|| "disconnected".to_owned())?;
    // The wait blocks, so it must not run on an async worker thread.
    tauri::async_runtime::spawn_blocking(move || {
        answer
            .recv_timeout(WRITE_WAIT)
            .map_err(|_| "deviceError:timeout".to_owned())?
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The values last applied to the device, `null` when there are none.
#[tauri::command]
fn get_device_prefs(devices: State<'_, DeviceManager>, id: String) -> Option<DevicePrefs> {
    devices.prefs(&id)
}

/// Shows and focuses the main window; with a device id the UI is told to
/// select that device.
#[tauri::command]
fn open_main_window(app: AppHandle, device_id: Option<String>) {
    flyout::hide(&app);
    tray::show_main_window(&app);
    if let Some(id) = device_id
        && let Err(e) = app.emit_to(tray::MAIN_WINDOW, SELECT_DEVICE_EVENT, id)
    {
        eprintln!("myhub: cannot emit {SELECT_DEVICE_EVENT}: {e}");
    }
}

#[tauri::command]
fn hide_flyout(app: AppHandle) {
    flyout::hide(&app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // The single-instance plugin must be registered first.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app);
        }))
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let state = SettingsState::load(config_dir.join(settings::SETTINGS_FILE));
            let current = state.get();
            app.manage(state);
            app.manage(flyout::FlyoutState::default());
            sync_autostart(app.handle(), current.launch_at_login);
            let autostarted = std::env::args().any(|a| a == AUTOSTART_ARG);

            let tray_ok = match tray::init(app, &current) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("myhub: tray icon unavailable: {e}");
                    false
                }
            };
            // Started after the tray so the first status update finds it.
            app.manage(DeviceManager::start(app.handle().clone()));
            // Without a tray there is no way to bring a hidden window back.
            if !(current.start_minimized || autostarted) || !tray_ok {
                tray::show_main_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            let app = window.app_handle();
            match window.label() {
                tray::MAIN_WINDOW => {
                    if let WindowEvent::ThemeChanged(theme) = event {
                        tray::set_theme(app, *theme);
                    }
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        let has_tray = app.try_state::<tray::TrayState>().is_some();
                        let close_to_tray = app
                            .try_state::<SettingsState>()
                            .is_some_and(|s| s.get().close_to_tray);
                        if close_to_tray && has_tray {
                            api.prevent_close();
                            let _ = window.hide();
                        } else {
                            // The hidden flyout window would keep the app
                            // alive after the main window is gone.
                            app.exit(0);
                        }
                    }
                }
                flyout::FLYOUT_WINDOW => match event {
                    WindowEvent::Focused(false) => flyout::on_blur(app),
                    // Never destroy it: it is shown again from the tray.
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        flyout::hide(app);
                    }
                    _ => {}
                },
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            get_settings,
            set_settings,
            list_devices,
            get_device_detail,
            refresh_device,
            set_dpi,
            set_report_rate,
            get_device_prefs,
            enter_temporary_mode,
            leave_temporary_mode,
            save_profile_to_mouse,
            open_main_window,
            hide_flyout
        ])
        .build(tauri::generate_context!())
        .expect("error while building MyHUB")
        .run(|app, event| {
            if let RunEvent::Exit = event
                && let Some(devices) = app.try_state::<DeviceManager>()
            {
                devices.shutdown();
            }
        });
}
