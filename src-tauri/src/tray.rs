//! System tray icon, its menu, and main window show/hide helpers.

use std::sync::{Mutex, MutexGuard};

use myhub_core::Connection;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Theme};

use crate::flyout;
use crate::settings::{Language, Settings, SettingsState, resolve_language};

pub const MAIN_WINDOW: &str = "main";
const TRAY_ID: &str = "main";
const ID_FLYOUT: &str = "flyout";
const ID_SHOW: &str = "show";
pub const ID_DEVICES_STATUS: &str = "devices-status";
/// Most device status rows the menu shows. Rows beyond the first sit in the menu only while
/// they have a line.
pub const MAX_STATUS_ROWS: usize = 3;
/// Menu index of the first extra status row: after flyout, show and the first status row.
const EXTRA_STATUS_POSITION: usize = 3;

/// Menu id of the extra status row `n` (1-based among the extra rows: row 1 is the second line).
fn extra_status_id(n: usize) -> String {
    format!("devices-status-{}", n + 1)
}
const ID_QUIT: &str = "quit";

/// Battery level at or below which the tray shows the low battery icon.
const LOW_BATTERY_PERCENT: u8 = 15;

/// What the tray icon says about the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayLook {
    Normal,
    Disconnected,
    LowBattery,
}

/// The taskbar the icon sits on: light taskbars need dark ink and the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarTheme {
    Light,
    Dark,
}

impl From<Theme> for TaskbarTheme {
    fn from(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self::Light,
            _ => Self::Dark,
        }
    }
}

/// Which tray icon is shown now: the OS theme and the device state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IconChoice {
    theme: TaskbarTheme,
    look: TrayLook,
}

/// The look for the device state: connection and last battery level, or `None` without a device.
pub fn look_for(device: Option<(Connection, Option<u8>)>) -> TrayLook {
    match device {
        Some((Connection::Disconnected, _)) => TrayLook::Disconnected,
        Some((Connection::Connected, Some(percent))) if percent <= LOW_BATTERY_PERCENT => {
            TrayLook::LowBattery
        }
        _ => TrayLook::Normal,
    }
}

// 32 px pixel art rasterized from assets/design/tray-*-32.svg.
static DARK_NORMAL: &[u8] = include_bytes!("../icons/tray/dark-normal-32.png");
static DARK_DISCONNECTED: &[u8] = include_bytes!("../icons/tray/dark-disconnected-32.png");
static DARK_LOW_BATTERY: &[u8] = include_bytes!("../icons/tray/dark-lowbattery-32.png");
static LIGHT_NORMAL: &[u8] = include_bytes!("../icons/tray/light-normal-32.png");
static LIGHT_DISCONNECTED: &[u8] = include_bytes!("../icons/tray/light-disconnected-32.png");
static LIGHT_LOW_BATTERY: &[u8] = include_bytes!("../icons/tray/light-lowbattery-32.png");

fn icon_bytes(choice: IconChoice) -> &'static [u8] {
    match (choice.theme, choice.look) {
        (TaskbarTheme::Dark, TrayLook::Normal) => DARK_NORMAL,
        (TaskbarTheme::Dark, TrayLook::Disconnected) => DARK_DISCONNECTED,
        (TaskbarTheme::Dark, TrayLook::LowBattery) => DARK_LOW_BATTERY,
        (TaskbarTheme::Light, TrayLook::Normal) => LIGHT_NORMAL,
        (TaskbarTheme::Light, TrayLook::Disconnected) => LIGHT_DISCONNECTED,
        (TaskbarTheme::Light, TrayLook::LowBattery) => LIGHT_LOW_BATTERY,
    }
}

struct Texts {
    flyout: &'static str,
    show: &'static str,
    no_devices: &'static str,
    quit: &'static str,
}

fn texts(language: Language) -> Texts {
    match language {
        Language::Pl => Texts {
            flyout: "Szybki panel",
            show: "Pokaż MyHUB",
            no_devices: "Brak urządzeń",
            quit: "Zakończ",
        },
        _ => Texts {
            flyout: "Quick panel",
            show: "Show MyHUB",
            no_devices: "No devices",
            quit: "Quit",
        },
    }
}

fn effective_language(settings: &Settings) -> Language {
    resolve_language(settings.language, sys_locale::get_locale().as_deref())
}

/// The language the UI texts use now: the setting, with `auto` resolved from the OS.
pub fn current_language(app: &AppHandle) -> Language {
    let settings = app
        .try_state::<SettingsState>()
        .map(|state| state.get())
        .unwrap_or_default();
    effective_language(&settings)
}

/// Managed state present only when the tray icon was created successfully.
/// Holds the menu item handles so texts can be updated later.
pub struct TrayState {
    tray: TrayIcon,
    flyout: MenuItem<tauri::Wry>,
    show: MenuItem<tauri::Wry>,
    devices_status: MenuItem<tauri::Wry>,
    /// Status rows after the first; row `i` is in the menu only while its flag is set. The shown
    /// rows are always a prefix.
    extra_status: Vec<(MenuItem<tauri::Wry>, Mutex<bool>)>,
    menu: Menu<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
    /// Text set by the device layer; `None` means the localized "No devices".
    devices_text: Mutex<Option<String>>,
    /// The icon variant in use. The theme is cached because asking the window for it blocks on the
    /// main thread, which the device worker must not do.
    icon: Mutex<IconChoice>,
}

impl TrayState {
    fn lock_text(&self) -> MutexGuard<'_, Option<String>> {
        self.devices_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Main thread only: sets the status rows after the first one from `lines`, inserting an item
    /// below the previous row when it is not in the menu yet, or removing it when it has no line.
    fn show_extra(&self, lines: &[String]) {
        for (i, (item, shown)) in self.extra_status.iter().enumerate() {
            let mut shown = shown
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match lines.get(i) {
                Some(text) => {
                    let _ = item.set_text(text);
                    if !*shown {
                        if self.menu.insert(item, EXTRA_STATUS_POSITION + i).is_err() {
                            // Later rows would land below the separator: keep the prefix.
                            break;
                        }
                        *shown = true;
                    }
                }
                None => {
                    if *shown && self.menu.remove(item).is_ok() {
                        *shown = false;
                    }
                }
            }
        }
    }

    fn lock_icon(&self) -> MutexGuard<'_, IconChoice> {
        self.icon
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Creates the tray icon and its menu and registers [`TrayState`].
pub fn init(app: &App, settings: &Settings) -> tauri::Result<()> {
    let t = texts(effective_language(settings));
    let flyout = MenuItemBuilder::with_id(ID_FLYOUT, t.flyout).build(app)?;
    let show = MenuItemBuilder::with_id(ID_SHOW, t.show).build(app)?;
    let devices_status = MenuItemBuilder::with_id(ID_DEVICES_STATUS, t.no_devices)
        .enabled(false)
        .build(app)?;
    let extra_status = (1..MAX_STATUS_ROWS)
        .map(|n| {
            MenuItemBuilder::with_id(extra_status_id(n), "")
                .enabled(false)
                .build(app)
                .map(|item| (item, Mutex::new(false)))
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let quit = MenuItemBuilder::with_id(ID_QUIT, t.quit).build(app)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&flyout, &show, &devices_status, &separator, &quit])?;

    // The main window follows the OS app theme; a taskbar theme of its own is not exposed, so dark
    // is the default when the window or its theme is unavailable.
    let theme = app
        .get_webview_window(MAIN_WINDOW)
        .and_then(|window| window.theme().ok())
        .map_or(TaskbarTheme::Dark, TaskbarTheme::from);
    let choice = IconChoice {
        theme,
        look: TrayLook::Normal,
    };

    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(icon_bytes(choice))?)
        .tooltip("MyHUB")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_FLYOUT => flyout::show(app),
            ID_SHOW => show_main_window(app),
            ID_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Lets the positioner learn where the tray icon is.
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                flyout::toggle(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(TrayState {
        tray,
        flyout,
        show,
        devices_status,
        extra_status,
        menu,
        quit,
        devices_text: Mutex::new(None),
        icon: Mutex::new(choice),
    });
    Ok(())
}

/// Re-applies settings that affect the tray (menu language). No-op without a tray.
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let t = texts(effective_language(settings));
    let _ = state.flyout.set_text(t.flyout);
    let _ = state.show.set_text(t.show);
    let _ = state.quit.set_text(t.quit);
    if state.lock_text().is_none() {
        let _ = state.devices_status.set_text(t.no_devices);
    }
}

/// For the device layer: sets the text of the disabled `devices-status` item from the first of
/// `lines` (or restores the localized "No devices" when there is none) and of the extra rows
/// below it. An extra row sits in the menu only while it has a line; lines beyond
/// [`MAX_STATUS_ROWS`] are dropped. No-op without a tray.
pub fn set_devices_status(app: &AppHandle, mut lines: Vec<String>) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    lines.truncate(MAX_STATUS_ROWS);
    let mut lines = lines.into_iter();
    let text = lines.next();
    let extra: Vec<String> = lines.collect();
    let shown = match &text {
        Some(text) => text.clone(),
        None => {
            let settings = app.state::<crate::settings::SettingsState>().get();
            texts(effective_language(&settings)).no_devices.to_string()
        }
    };
    *state.lock_text() = text;
    // Posted, not awaited: the caller is the device worker, which must not
    // block on the main thread. The posts run in order, each with its own values.
    let item = state.devices_status.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = item.set_text(shown);
        if let Some(state) = handle.try_state::<TrayState>() {
            state.show_extra(&extra);
        }
    });
}

/// Applies `change` to the icon choice and, only when the variant differs, posts the new icon to the
/// main thread. Not awaited: callers include the device worker. No-op without a tray.
fn change_icon(app: &AppHandle, change: impl FnOnce(&mut IconChoice)) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let next = {
        let mut choice = state.lock_icon();
        let before = *choice;
        change(&mut choice);
        if *choice == before {
            return;
        }
        *choice
    };
    let Ok(image) = Image::from_bytes(icon_bytes(next)) else {
        return;
    };
    let tray = state.tray.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = tray.set_icon(Some(image));
    });
}

/// For the device layer: shows the icon for the device state.
pub fn set_look(app: &AppHandle, look: TrayLook) {
    change_icon(app, |choice| choice.look = look);
}

/// For the main window's theme change event: switches between the light and dark taskbar icons.
pub fn set_theme(app: &AppHandle, theme: Theme) {
    change_icon(app, |choice| choice.theme = theme.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_follows_connection_and_battery() {
        use Connection::{Connected, Disconnected};
        assert_eq!(look_for(None), TrayLook::Normal);
        assert_eq!(look_for(Some((Connected, None))), TrayLook::Normal);
        assert_eq!(look_for(Some((Connected, Some(55)))), TrayLook::Normal);
        assert_eq!(look_for(Some((Connected, Some(16)))), TrayLook::Normal);
        assert_eq!(look_for(Some((Connected, Some(15)))), TrayLook::LowBattery);
        assert_eq!(look_for(Some((Connected, Some(0)))), TrayLook::LowBattery);
        assert_eq!(
            look_for(Some((Disconnected, Some(5)))),
            TrayLook::Disconnected
        );
        assert_eq!(look_for(Some((Disconnected, None))), TrayLook::Disconnected);
    }

    #[test]
    fn every_variant_has_a_decodable_32px_icon() {
        for theme in [TaskbarTheme::Light, TaskbarTheme::Dark] {
            for look in [
                TrayLook::Normal,
                TrayLook::Disconnected,
                TrayLook::LowBattery,
            ] {
                let image = Image::from_bytes(icon_bytes(IconChoice { theme, look }))
                    .unwrap_or_else(|e| panic!("{theme:?} {look:?}: {e}"));
                assert_eq!((image.width(), image.height()), (32, 32));
            }
        }
    }

    #[test]
    fn theme_maps_to_taskbar() {
        assert_eq!(TaskbarTheme::from(Theme::Light), TaskbarTheme::Light);
        assert_eq!(TaskbarTheme::from(Theme::Dark), TaskbarTheme::Dark);
    }
}
