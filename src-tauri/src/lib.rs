//! Tauri shell: owns the backends, exposes them as commands, forwards
//! backend broadcasts as window events. No file logic lives here.

mod commands;
mod prompter;
mod state;

use tauri::{Emitter, Manager};
use tokio::sync::broadcast;

use state::AppState;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = AppState::new(app.handle().clone());
            forward(app.handle().clone(), "conn:status", state.ssh.subscribe());
            forward(app.handle().clone(), "transfer:update", state.transfers.subscribe());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::connect_host,
            commands::disconnect_host,
            commands::local_places,
            commands::home_dir,
            commands::list_dir,
            commands::stat_path,
            commands::disk_free,
            commands::create_dir,
            commands::rename_path,
            commands::delete_paths,
            commands::read_text,
            commands::open_external,
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
