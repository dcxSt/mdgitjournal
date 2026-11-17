//! The Tauri shell: app state, commands and events over the mdgj-* crates.

mod commands;
mod error;
#[cfg(target_os = "macos")]
mod menu;
mod state;
mod view;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu::build).on_menu_event(menu::on_event);
    builder
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // If the journal can't be opened, the window still is, and the UI says why.
            let backend: state::Backend = state::AppState::open(app.handle());
            app.manage(backend);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::get_settings,
            commands::set_settings,
            commands::list_entries,
            commands::read_entry,
            commands::write_entry,
            commands::create_entry,
            commands::rename_entry,
            commands::delete_entry,
            commands::search,
            commands::import_image,
            commands::sync_state,
            commands::fetch,
            commands::pull,
            commands::commit,
            commands::commit_and_push,
            commands::auth_status,
            commands::github_device_start,
            commands::github_device_poll,
            commands::github_set_pat,
            commands::github_list_repos,
            commands::github_connect,
            commands::github_disconnect,
            commands::github_urls,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mdgitjournal");
}
