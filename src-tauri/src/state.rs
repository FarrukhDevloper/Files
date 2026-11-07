use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;
use vpsfiles_ssh::SshManager;
use vpsfiles_stream::MediaServer;
use vpsfiles_transfer::TransferManager;
use vpsfiles_vfs::local::LocalBackend;
use vpsfiles_vfs::{Backend, BackendResolver, Result, SourceId};

use crate::prompter::UiPrompter;

/// How many transfer jobs run at once; the rest queue.
const PARALLEL_TRANSFERS: usize = 3;

pub struct AppState {
    pub resolver: Arc<Resolver>,
    pub ssh: Arc<SshManager>,
    pub transfers: TransferManager,
    pub prompter: Arc<UiPrompter>,
    /// Started on first `media_url` call — no port is opened until needed.
    pub media: OnceCell<MediaServer>,
}

impl AppState {
    pub fn new(app: tauri::AppHandle) -> Self {
        let prompter = Arc::new(UiPrompter::new(app));
        let ssh = Arc::new(SshManager::new(prompter.clone()));
        let resolver = Arc::new(Resolver { local: Arc::new(LocalBackend::new()), ssh: ssh.clone() });
        let transfers = TransferManager::new(resolver.clone(), PARALLEL_TRANSFERS);
        Self { resolver, ssh, transfers, prompter, media: OnceCell::new() }
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
