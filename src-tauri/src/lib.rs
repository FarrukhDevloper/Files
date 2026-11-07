//! Tauri shell: owns the backends, exposes them as commands, forwards
//! backend broadcasts as window events. No file logic lives here.

mod commands;
mod open;
mod players;
mod prompter;
mod state;

use tauri::{Emitter, Manager};
use tokio::sync::broadcast;

use state::AppState;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let state = AppState::new(app.handle().clone());
            forward(app.handle().clone(), "conn:status", state.ssh.subscribe());
            forward(app.handle().clone(), "transfer:update", state.transfers.subscribe());
            forward(app.handle().clone(), "edit:update", state.edits.subscribe());
            app.manage(state);
            #[cfg(debug_assertions)]
            dev_query(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::connect_host,
            commands::disconnect_host,
            commands::save_host,
            commands::remove_host,
            commands::forget_password,
            commands::ssh_config_path,
            commands::local_places,
            commands::home_dir,
            commands::list_dir,
            commands::stat_path,
            commands::disk_free,
            commands::create_dir,
            commands::rename_path,
            commands::delete_paths,
            commands::read_text,
            open::open_external,
            open::edit_list,
            open::edit_stop,
            open::edit_resolve,
            open::media_base,
            open::external_players,
            open::open_in_player,
            commands::transfer_start,
            commands::transfer_list,
            commands::transfer_pause,
            commands::transfer_resume,
            commands::transfer_cancel,
            commands::transfer_clear_finished,
            commands::media_url,
            commands::prompt_respond,
        ])
        .run(tauri::generate_context!())
        .expect("error while running VPS Files");
}

/// Re-emit every broadcast message as a window event named `event`.
fn forward<T>(app: tauri::AppHandle, event: &'static str, mut rx: broadcast::Receiver<T>)
where
    T: serde::Serialize + Clone + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    let _ = app.emit(event, msg);
                }
                // A slow UI only misses intermediate progress ticks.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Debug builds only: `VPSFILES_DEV_QUERY="open=local:/tmp&view=icons"` reloads
/// the window with that query so the UI's dev hooks (src/lib/devhooks.ts) can
/// put it in a given state — used for headless screenshot checks.
#[cfg(debug_assertions)]
fn dev_query(app: &tauri::App) {
    let Ok(query) = std::env::var("VPSFILES_DEV_QUERY") else { return };
    let Some(window) = app.get_webview_window("main") else { return };
    if let Ok(mut url) = window.url() {
        url.set_query(Some(&query));
        let _ = window.navigate(url);
    }
}
