//! Bridges SSH prompts to the UI: emit `prompt:request`, park a oneshot,
//! resolve it when the UI calls `prompt_respond`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::oneshot;
use vpsfiles_ssh::Prompter;

/// An unanswered dialog is treated as "cancel" after this long.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PromptRequest {
    HostKey { id: String, host: String, algorithm: String, fingerprint: String },
    Password { id: String, host: String, user: String },
    Passphrase { id: String, host: String, key_path: String },
}

pub struct Answer {
    pub accept: bool,
    pub secret: Option<String>,
}

pub struct UiPrompter {
    app: tauri::AppHandle,
    pending: Mutex<HashMap<String, oneshot::Sender<Answer>>>,
}

impl UiPrompter {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app, pending: Mutex::new(HashMap::new()) }
    }

    /// Deliver the UI's answer. `false` if the prompt already timed out.
    pub fn respond(&self, id: &str, answer: Answer) -> bool {
        match self.pending.lock().unwrap().remove(id) {
            Some(tx) => tx.send(answer).is_ok(),
            None => false,
        }
    }

    async fn ask(&self, make: impl FnOnce(String) -> PromptRequest) -> Option<Answer> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let answer = match self.app.emit("prompt:request", make(id.clone())) {
            Ok(()) => tokio::time::timeout(PROMPT_TIMEOUT, rx).await.ok().and_then(Result::ok),
            Err(_) => None,
        };
        self.pending.lock().unwrap().remove(&id);
        answer
    }
}

#[async_trait]
impl Prompter for UiPrompter {
    async fn confirm_host_key(&self, host: &str, algorithm: &str, fingerprint: &str) -> bool {
        self.ask(|id| PromptRequest::HostKey {
            id,
            host: host.into(),
            algorithm: algorithm.into(),
            fingerprint: fingerprint.into(),
        })
        .await
        .is_some_and(|a| a.accept)
    }

    async fn password(&self, host: &str, user: &str) -> Option<String> {
        let a = self.ask(|id| PromptRequest::Password { id, host: host.into(), user: user.into() }).await?;
        a.accept.then_some(a.secret).flatten()
    }

    async fn passphrase(&self, host: &str, key_path: &str) -> Option<String> {
        let a = self
            .ask(|id| PromptRequest::Passphrase { id, host: host.into(), key_path: key_path.into() })
            .await?;
        a.accept.then_some(a.secret).flatten()
    }
}
