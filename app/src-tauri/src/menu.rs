//! The default macOS menu, except that Quit closes the window first.
//!
//! The standard Quit item ends the app at once, which would lose edits the UI has not saved yet.
//! Closing the window lets the UI save them, or keep the window open if saving fails; closing the
//! last window then quits.

use tauri::menu::{Menu, MenuEvent, MenuItem, MenuItemKind};
use tauri::{AppHandle, Manager, Wry};

const QUIT: &str = "quit";

pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::default(app)?;
    // The first submenu is the app menu, and its last item is Quit.
    if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.first() {
        let last = app_menu.items()?.len().saturating_sub(1);
        app_menu.remove_at(last)?;
        let label = format!("Quit {}", app.package_info().name);
        app_menu.append(&MenuItem::with_id(
            app,
            QUIT,
            label,
            true,
            Some("CmdOrCtrl+Q"),
        )?)?;
    }
    Ok(menu)
}

pub fn on_event(app: &AppHandle, event: MenuEvent) {
    if event.id() == QUIT {
        for window in app.webview_windows().into_values() {
            // Fails only if the window is already gone.
            let _ = window.close();
        }
    }
}
