use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;
use vpsfiles_edit::EditManager;
use vpsfiles_ssh::SshManager;
use vpsfiles_stream::MediaServer;
use vpsfiles_transfer::TransferManager;
use vpsfiles_vfs::local::LocalBackend;
use vpsfiles_vfs::{Backend, BackendResolver, Result, SourceId, VfsError};

use crate::prompter::UiPrompter;

/// How many transfer jobs run at once; the rest queue.
const PARALLEL_TRANSFERS: usize = 3;

pub struct AppState {
    pub resolver: Arc<Resolver>,
    pub ssh: Arc<SshManager>,
    pub transfers: TransferManager,
    pub prompter: Arc<UiPrompter>,
    pub edits: EditManager,
    /// Started on first use — no port is opened until needed.
    media: OnceCell<MediaServer>,
}

impl AppState {
    pub fn new(app: tauri::AppHandle) -> Self {
        let prompter = Arc::new(UiPrompter::new(app));
        let ssh = Arc::new(SshManager::new(prompter.clone()));
        let resolver = Arc::new(Resolver { local: Arc::new(LocalBackend::new()), ssh: ssh.clone() });
        let transfers = TransferManager::new(resolver.clone(), PARALLEL_TRANSFERS);
        let edit_root = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("vps-files").join("edit");
        let edits = EditManager::new(
            resolver.clone(),
            edit_root,
            Arc::new(|p: &std::path::Path| {
                tauri_plugin_opener::open_path(p, None::<&str>).map_err(|e| VfsError::Other(e.to_string()))
            }),
        );
        Self { resolver, ssh, transfers, prompter, edits, media: OnceCell::new() }
    }

    pub async fn media(&self) -> Result<&MediaServer> {
        self.media.get_or_try_init(|| MediaServer::start(self.resolver.clone())).await
    }

    pub async fn backend(&self, source: &SourceId) -> Result<Arc<dyn Backend>> {
        self.resolver.backend(source).await
    }
}

pub struct Resolver {
    local: Arc<LocalBackend>,
    ssh: Arc<SshManager>,
}

#[async_trait]
impl BackendResolver for Resolver {
    async fn backend(&self, source: &SourceId) -> Result<Arc<dyn Backend>> {
        match source {
            SourceId::Local => Ok(self.local.clone()),
            SourceId::Ssh { host } => self.ssh.backend(host).await,
        }
    }
}
