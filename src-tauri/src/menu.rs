//! The macOS application menu.
//!
//! Tauri's default menu gives "About Aria" the stock AppKit panel, which can
//! show nothing but a name and a version, and has no "Check for Updates…" or
//! "Settings…". This is that default menu with Aria's own items in the app
//! submenu. The items do no work here: each one brings the main window
//! forward and emits an event, and the frontend (`src/hooks/useAppMenu.ts`)
//! opens the About dialog, starts a manual update check or opens Settings,
//! because the updater and the About screen both live in the webview.
//!
//! Windows and Linux have no menu bar (the window is a narrow phone) and no
//! tray, so there About and the update check are reached from Settings.

use tauri::menu::{
    Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};

const ABOUT_ID: &str = "aria.about";
const CHECK_UPDATES_ID: &str = "aria.check-updates";
const SETTINGS_ID: &str = "aria.settings";

/// The events the frontend listens for. Keep in step with
/// `src/hooks/useAppMenu.ts`.
const EVENT_ABOUT: &str = "menu-about";
const EVENT_CHECK_UPDATES: &str = "menu-check-updates";
const EVENT_SETTINGS: &str = "menu-settings";

/// The frontend event for one of Aria's own menu items, or `None` for the
/// predefined ones (Hide, Quit, Copy, ...), which AppKit handles itself.
fn frontend_event(menu_id: &str) -> Option<&'static str> {
    match menu_id {
        ABOUT_ID => Some(EVENT_ABOUT),
        CHECK_UPDATES_ID => Some(EVENT_CHECK_UPDATES),
        SETTINGS_ID => Some(EVENT_SETTINGS),
        _ => None,
    }
}

/// Builds the menu bar: Tauri's default, with About, Check for Updates… and
/// Settings… in the app submenu.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let name = app.package_info().name.clone();

    let app_menu = Submenu::with_items(
        app,
        &name,
        true,
        &[
            &MenuItem::with_id(app, ABOUT_ID, format!("About {name}"), true, None::<&str>)?,
            &MenuItem::with_id(app, CHECK_UPDATES_ID, "Check for Updates…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            // ⌘, is the shortcut the webview already handles; owning it here
            // means it also works while focus is in the Developer window.
            &MenuItem::with_id(app, SETTINGS_ID, "Settings…", true, Some("CmdOrCtrl+,"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            // The predefined Quit, as in the default menu: it is what sends
            // `ExitRequested`, which unregisters every account before exiting.
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[&PredefinedMenuItem::close_window(app, None)?],
    )?;

    // Without an Edit menu, ⌘C / ⌘V / ⌘A do nothing in a macOS webview.
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;

    let window_menu = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    // Empty, as in the default menu: macOS adds its own search field to a
    // menu with this id.
    let help_menu = Submenu::with_id_and_items(app, HELP_SUBMENU_ID, "Help", true, &[])?;

    Menu::with_items(
        app,
        &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu, &help_menu],
    )
}

/// Handles a click on one of Aria's own menu items.
pub fn handle_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let Some(name) = frontend_event(event.id().as_ref()) else {
        return;
    };
    // The About dialog and Settings live in the main window, which may be
    // hidden, minimized or behind the Developer window.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    if let Err(e) = app.emit_to("main", name, ()) {
        log::warn!("Could not forward menu item {:?}: {}", event.id(), e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_items_map_to_frontend_events() {
        assert_eq!(frontend_event(ABOUT_ID), Some("menu-about"));
        assert_eq!(frontend_event(CHECK_UPDATES_ID), Some("menu-check-updates"));
        assert_eq!(frontend_event(SETTINGS_ID), Some("menu-settings"));
    }

    #[test]
    fn predefined_items_are_left_to_appkit() {
        assert_eq!(frontend_event("quit"), None);
        assert_eq!(frontend_event(""), None);
    }
}
