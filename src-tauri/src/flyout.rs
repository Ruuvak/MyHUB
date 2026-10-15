//! The quick panel ("flyout"): a small frameless window near the tray icon.
//!
//! A tray click toggles it; it hides itself when it loses focus. Two races
//! need a guard:
//!
//! * Showing the window and focusing it can produce an immediate blur (the
//!   tray menu or the click itself still owns the focus). A blur within
//!   [`SHOW_BLUR_GUARD`] of the show is ignored.
//! * Clicking the tray icon while the flyout is open blurs it first (mouse
//!   down hides it), and the click that follows would show it again. A tray
//!   click within [`HIDE_CLICK_GUARD`] of a blur-hide counts as the click that
//!   closed it and does nothing more.

use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_positioner::{Position, WindowExt};

pub const FLYOUT_WINDOW: &str = "flyout";

/// A blur this soon after showing the flyout is not the user clicking away.
const SHOW_BLUR_GUARD: Duration = Duration::from_millis(200);
/// A tray click this soon after a blur-hide is the click that caused it.
const HIDE_CLICK_GUARD: Duration = Duration::from_millis(350);

#[derive(Default)]
struct Marks {
    shown_at: Option<Instant>,
    blur_hidden_at: Option<Instant>,
}

/// Managed state of the flyout.
#[derive(Default)]
pub struct FlyoutState {
    marks: Mutex<Marks>,
}

impl FlyoutState {
    fn marks(&self) -> MutexGuard<'_, Marks> {
        self.marks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(FLYOUT_WINDOW)
}

/// Above the tray icon, kept inside the screen. Falls back to the bottom right
/// corner when the tray position is unknown (Linux tray icons report none) or
/// on platforms where the tray rectangle is not used.
fn place(window: &WebviewWindow) {
    // Position::TrayCenter puts the window directly above the icon (below it
    // when there is no room above); TrayBottomRight would put the top left
    // corner at the icon's top right and push the window off the screen.
    let placed = cfg!(windows) && window.move_window_constrained(Position::TrayCenter).is_ok();
    if !placed && let Err(e) = window.move_window(Position::BottomRight) {
        eprintln!("myhub: cannot position the flyout: {e}");
    }
}

/// Shows the flyout near the tray and focuses it.
pub fn show(app: &AppHandle) {
    let Some(window) = window(app) else {
        return;
    };
    place(&window);
    if let Some(state) = app.try_state::<FlyoutState>() {
        let mut marks = state.marks();
        marks.shown_at = Some(Instant::now());
        marks.blur_hidden_at = None;
    }
    let _ = window.show();
    let _ = window.set_focus();
    schedule_guard_check(app);
}

/// At the end of the show guard a blur that was swallowed would leave the
/// flyout open without focus. One-shot timer thread, the check runs on the
/// main thread.
fn schedule_guard_check(app: &AppHandle) {
    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("flyout-guard".into())
        .spawn(move || {
            thread::sleep(SHOW_BLUR_GUARD);
            let on_main = app.clone();
            let _ = app.run_on_main_thread(move || guard_expired(&on_main));
        });
    if let Err(e) = spawned {
        eprintln!("myhub: cannot start the flyout guard timer: {e}");
    }
}

/// Whether the guard expiry must hide the flyout: it is open but has no focus.
fn hides_at_guard_expiry(visible: bool, focused: bool) -> bool {
    visible && !focused
}

fn guard_expired(app: &AppHandle) {
    let Some(window) = window(app) else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(true);
    if hides_at_guard_expiry(visible, focused) {
        // Same path as a real blur; the guard has elapsed by now. A newer
        // show restarted the guard and its own timer takes over.
        on_blur(app);
    }
}

/// Hides the flyout.
pub fn hide(app: &AppHandle) {
    if let Some(window) = window(app) {
        let _ = window.hide();
    }
}

/// Tray click: hide when visible, show otherwise.
pub fn toggle(app: &AppHandle) {
    let Some(window) = window(app) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        hide(app);
        return;
    }
    if let Some(state) = app.try_state::<FlyoutState>() {
        let mut marks = state.marks();
        if marks
            .blur_hidden_at
            .take()
            .is_some_and(|at| at.elapsed() < HIDE_CLICK_GUARD)
        {
            // This click already closed it by taking the focus.
            return;
        }
    }
    show(app);
}

/// The flyout lost focus: hide it, unless it was only just shown.
pub fn on_blur(app: &AppHandle) {
    // Hiding the window also reports a blur; only a visible one is clicked away.
    if !window(app).is_some_and(|w| w.is_visible().unwrap_or(false)) {
        return;
    }
    if let Some(state) = app.try_state::<FlyoutState>() {
        let mut marks = state.marks();
        if marks
            .shown_at
            .is_some_and(|at| at.elapsed() < SHOW_BLUR_GUARD)
        {
            return;
        }
        marks.blur_hidden_at = Some(Instant::now());
    }
    hide(app);
}

#[cfg(test)]
mod tests {
    use super::hides_at_guard_expiry;

    #[test]
    fn guard_expiry_hides_only_a_visible_unfocused_flyout() {
        assert!(hides_at_guard_expiry(true, false));
        assert!(!hides_at_guard_expiry(true, true));
        assert!(!hides_at_guard_expiry(false, false));
        assert!(!hides_at_guard_expiry(false, true));
    }
}
