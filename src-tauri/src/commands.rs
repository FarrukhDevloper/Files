//! Every `invoke()` target. Thin: validate, resolve the backend, delegate.
//! Contract: docs/IPC.md and src/lib/ipc/types.ts.

use serde::Serialize;
use tauri::State;
use tokio::io::AsyncReadExt;
use vpsfiles_ssh::{ConnStatus, HostConfig, HostDraft, ServerInfo};
use vpsfiles_transfer::{JobId, JobSnapshot, TransferRequest};
use vpsfiles_vfs::{local, path, DiskSpace, Entry, Location, Place, Result, SourceId, VfsError};

use crate::prompter::Answer;
use crate::state::AppState;

/// Hard cap for `read_text`, whatever the UI asks for.
const MAX_TEXT_PREVIEW: u64 = 2 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    #[serde(flatten)]
    config: HostConfig,
    status: ConnStatus,
}

#[derive(Serialize)]
pub struct TextPreview {
    text: String,
    truncated: bool,
}

// ---- hosts -----------------------------------------------------------------

#[tauri::command]
pub fn list_hosts(state: State<'_, AppState>) -> Result<Vec<HostInfo>> {
    Ok(state
        .ssh
        .hosts()?
        .into_iter()
        .map(|config| HostInfo { status: state.ssh.status(&config.alias), config })
        .collect())
}

/// Create (`previous_alias` = None) or update an app-managed server.
#[tauri::command]
pub async fn save_host(
    state: State<'_, AppState>,
    draft: HostDraft,
    previous_alias: Option<String>,
) -> Result<HostInfo> {
    let config = state.ssh.save_host(draft, previous_alias.as_deref()).await?;
    Ok(HostInfo { status: state.ssh.status(&config.alias), config })
}

#[tauri::command]
pub async fn remove_host(state: State<'_, AppState>, alias: String) -> Result<()> {
    state.ssh.remove_host(&alias).await
}

#[tauri::command]
pub async fn forget_password(state: State<'_, AppState>, alias: String) -> Result<()> {
    state.ssh.forget_password(&alias).await
}

#[tauri::command]
pub fn ssh_config_path(state: State<'_, AppState>) -> String {
    state.ssh.config_path().display().to_string()
}

/// `interactive: false` (auto-connect) never prompts; it fails with
/// `sign_in_needed` instead.
#[tauri::command]
pub async fn connect_host(state: State<'_, AppState>, alias: String, interactive: Option<bool>) -> Result<()> {
    state.ssh.connect(&alias, interactive.unwrap_or(true)).await
}

#[tauri::command]
pub async fn server_info(state: State<'_, AppState>, alias: String) -> Result<ServerInfo> {
    state.ssh.server_info(&alias).await
}

#[tauri::command]
pub async fn disconnect_host(state: State<'_, AppState>, alias: String) -> Result<()> {
    state.ssh.disconnect(&alias).await;
    Ok(())
}

// ---- browsing --------------------------------------------------------------

#[tauri::command]
pub fn local_places() -> Vec<Place> {
    local::places()
}

#[tauri::command]
pub async fn home_dir(state: State<'_, AppState>, source: SourceId) -> Result<String> {
    state.backend(&source).await?.home().await
}

#[tauri::command]
pub async fn list_dir(state: State<'_, AppState>, location: Location) -> Result<Vec<Entry>> {
    state.backend(&location.source).await?.list(&location.path).await
}

#[tauri::command]
pub async fn stat_path(state: State<'_, AppState>, location: Location) -> Result<Entry> {
    state.backend(&location.source).await?.stat(&location.path).await
}

#[tauri::command]
pub async fn disk_free(state: State<'_, AppState>, location: Location) -> Result<Option<DiskSpace>> {
    state.backend(&location.source).await?.disk_space(&location.path).await
}

#[tauri::command]
pub async fn create_dir(state: State<'_, AppState>, location: Location) -> Result<Entry> {
    path::validate_name(path::file_name(&location.path))?;
    let fs = state.backend(&location.source).await?;
    fs.mkdir(&location.path).await?;
    fs.stat(&location.path).await
}

#[tauri::command]
pub async fn rename_path(
    state: State<'_, AppState>,
    location: Location,
    new_name: String,
) -> Result<Entry> {
    let parent = path::parent(&location.path)
        .ok_or_else(|| VfsError::InvalidName("cannot rename the root".into()))?;
    let to = path::join(&parent, &new_name)?;
    let fs = state.backend(&location.source).await?;
    fs.rename(&location.path, &to).await?;
    fs.stat(&to).await
}

#[tauri::command]
pub async fn delete_paths(
    state: State<'_, AppState>,
    locations: Vec<Location>,
    permanent: bool,
) -> Result<()> {
    for loc in locations {
        if path::is_root(&loc.path) {
            return Err(VfsError::PermissionDenied(format!("refusing to delete {}", loc.path)));
        }
        let fs = state.backend(&loc.source).await?;
        if !permanent && fs.capabilities().trash {
            fs.trash(&loc.path).await?;
        } else {
            fs.remove_all(&loc.path).await?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn read_text(
    state: State<'_, AppState>,
    location: Location,
    max_bytes: u64,
) -> Result<TextPreview> {
    let limit = max_bytes.min(MAX_TEXT_PREVIEW);
    let fs = state.backend(&location.source).await?;
    let entry = fs.stat(&location.path).await?;
    let mut buf = Vec::with_capacity(limit.min(entry.size) as usize);
    fs.open_read(&location.path, 0)
        .await?
        .take(limit)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| VfsError::from_io(e, &location.path))?;
    if buf.iter().take(8192).any(|&b| b == 0) {
        return Err(VfsError::Unsupported("binary file".into()));
    }
    Ok(TextPreview {
        truncated: entry.size > buf.len() as u64,
        text: String::from_utf8_lossy(&buf).into_owned(),
    })
}

// ---- transfers -------------------------------------------------------------

/// `async` so it runs on the tokio runtime the job worker is spawned onto.
#[tauri::command]
pub async fn transfer_start(state: State<'_, AppState>, request: TransferRequest) -> Result<JobId> {
    state.transfers.start(request)
}

#[tauri::command]
pub fn transfer_list(state: State<'_, AppState>) -> Vec<JobSnapshot> {
    state.transfers.list()
}

#[tauri::command]
pub fn transfer_pause(state: State<'_, AppState>, id: String) -> Result<()> {
    state.transfers.pause(&id)
}

#[tauri::command]
pub fn transfer_resume(state: State<'_, AppState>, id: String) -> Result<()> {
    state.transfers.resume(&id)
}

#[tauri::command]
pub fn transfer_cancel(state: State<'_, AppState>, id: String) -> Result<()> {
    state.transfers.cancel(&id)
}

#[tauri::command]
pub fn transfer_clear_finished(state: State<'_, AppState>) {
    state.transfers.clear_finished()
}

// ---- media + prompts -------------------------------------------------------

#[tauri::command]
pub async fn media_url(state: State<'_, AppState>, location: Location) -> Result<String> {
    Ok(state.media().await?.url_for(&location))
}

#[tauri::command]
pub fn prompt_respond(
    state: State<'_, AppState>,
    id: String,
    accept: bool,
    secret: Option<String>,
    remember: bool,
) -> Result<()> {
    if state.prompter.respond(&id, Answer { accept, secret, remember }) {
        Ok(())
    } else {
        Err(VfsError::NotFound(format!("prompt {id} expired")))
    }
}
