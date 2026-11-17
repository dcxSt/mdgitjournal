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
    // SAFETY: the first thing the app does, before any thread uses libgit2.
    unsafe { state::limit_git_waits() };
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
            commands::third_party_notices,
        ])
        .run(tauri::generate_context!())
        .expect("error while running mdgitjournal");
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    /// capabilities/default.json lets the window use what app/src/lib/api.ts needs of Tauri, and
    /// nothing else: no menu, tray, path, app or emitting events.
    #[test]
    fn the_window_may_only_do_what_the_ui_needs() {
        let app = mock_builder()
            .build(tauri::generate_context!(test = true))
            .unwrap();
        let window = WebviewWindowBuilder::new(&app, "main", WebviewUrl::default())
            .build()
            .unwrap();
        let call = |cmd: &str, args: Value| {
            get_ipc_response(
                &window,
                InvokeRequest {
                    cmd: cmd.into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: "tauri://localhost".parse().unwrap(),
                    body: InvokeBody::Json(args),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.into(),
                },
            )
        };
        let denied = |cmd: &str, args: Value| match call(cmd, args) {
            Err(Value::String(err)) => err.contains("not allowed"),
            _ => false,
        };

        // The app's events, focus, and the close request the UI saves on.
        let event = "tauri://close-requested";
        let id = call(
            "plugin:event|listen",
            json!({ "event": event, "target": { "kind": "Any" }, "handler": 2 }),
        )
        .unwrap()
        .deserialize::<u32>()
        .unwrap();
        call(
            "plugin:event|unlisten",
            json!({ "event": event, "eventId": id }),
        )
        .unwrap();

        assert!(denied("plugin:app|version", json!({})));
        assert!(denied(
            "plugin:path|resolve_directory",
            json!({ "directory": 1 })
        ));
        assert!(denied(
            "plugin:event|emit",
            json!({ "event": "sync://state" })
        ));
        assert!(denied("plugin:tray|new", json!({ "options": {} })));
        // E.g. a page script putting back the stock Quit, which skips saving.
        assert!(denied("plugin:menu|create_default", json!({})));

        // Closing once the UI has saved; last, since it closes the window.
        call("plugin:window|destroy", json!({ "label": "main" })).unwrap();
    }
}
