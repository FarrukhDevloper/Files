//! Opening files outside the app: OS default app (with sync-back for remote
//! files) and external media players.

use tauri::State;
use vpsfiles_edit::{EditSession, Resolve};
use vpsfiles_vfs::{local, Location, Result, SourceId, VfsError};

use crate::players;
use crate::state::AppState;

/// Local files open in place (`None`). Remote files get an edit session:
/// downloaded, opened, and every save uploaded back.
#[tauri::command]
pub async fn open_external(state: State<'_, AppState>, location: Location) -> Result<Option<EditSession>> {
    if location.source == SourceId::Local {
        tauri_plugin_opener::open_path(local::to_native(&location.path), None::<&str>)
            .map_err(|e| VfsError::Other(e.to_string()))?;
        return Ok(None);
    }
    state.edits.open(location).await.map(Some)
}

#[tauri::command]
pub fn edit_list(state: State<'_, AppState>) -> Vec<EditSession> {
    state.edits.list()
}

#[tauri::command]
pub async fn edit_stop(state: State<'_, AppState>, id: String) -> Result<()> {
    state.edits.stop(&id).await
}

#[tauri::command]
pub async fn edit_resolve(state: State<'_, AppState>, id: String, action: Resolve) -> Result<()> {
    state.edits.resolve(&id, action).await
}

/// `http://127.0.0.1:PORT/TOKEN` — the UI builds media/thumbnail URLs from it.
#[tauri::command]
pub async fn media_base(state: State<'_, AppState>) -> Result<String> {
    Ok(state.media().await?.base_url())
}

#[tauri::command]
pub fn external_players() -> Vec<String> {
    players::installed().into_iter().map(|p| p.name.to_string()).collect()
}

/// Stream `location` in an external player: `player` by name, else the
/// first one installed.
#[tauri::command]
pub async fn open_in_player(
    state: State<'_, AppState>,
    location: Location,
    player: Option<String>,
) -> Result<()> {
    let installed = players::installed();
    let chosen = match &player {
        Some(name) => installed.iter().find(|p| p.name.eq_ignore_ascii_case(name)),
        None => installed.first(),
    }
    .ok_or_else(|| VfsError::Unsupported("no external media player found (install mpv or VLC)".into()))?;
    let url = state.media().await?.url_for(&location);
    let mut child = std::process::Command::new(&chosen.exe)
        .arg(url)
        .spawn()
        .map_err(|e| VfsError::Other(format!("{}: {e}", chosen.name)))?;
    // Reap it when it exits so it doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
