//! `git_info` / `git_log`: local folders use the git CLI here, server
//! folders run the same commands through one SSH exec.

use std::sync::Arc;

use async_trait::async_trait;
use tauri::State;
use vpsfiles_git::{GitCommit, GitExec, GitInfo, LocalGit, Output};
use vpsfiles_ssh::SshManager;
use vpsfiles_vfs::{path, EntryKind, Location, Result, SourceId};

use crate::state::AppState;

struct RemoteGit {
    ssh: Arc<SshManager>,
    alias: String,
}

#[async_trait]
impl GitExec for RemoteGit {
    async fn run(&self, cwd: &str, cmds: &[Vec<String>]) -> Result<Option<Vec<Output>>> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let script = vpsfiles_git::remote_script(cwd, cmds, &token);
        let out = self.ssh.exec(&self.alias, &vpsfiles_git::sh_command(&script)).await?;
        Ok(vpsfiles_git::parse_remote(out.exit_status, &out.stdout, &token, cmds.len()))
    }
}

fn runner(state: &AppState, source: &SourceId) -> Box<dyn GitExec> {
    match source {
        SourceId::Local => Box::new(LocalGit),
        SourceId::Ssh { host } => Box::new(RemoteGit { ssh: state.ssh.clone(), alias: host.clone() }),
    }
}

#[tauri::command]
pub async fn git_info(state: State<'_, AppState>, location: Location) -> Result<Option<GitInfo>> {
    vpsfiles_git::info(runner(&state, &location.source).as_ref(), &location.path).await
}

#[tauri::command]
pub async fn git_log(state: State<'_, AppState>, location: Location, limit: u32) -> Result<Vec<GitCommit>> {
    let entry = state.backend(&location.source).await?.stat(&location.path).await?;
    let (cwd, spec) = match (entry.kind, path::parent(&location.path)) {
        (EntryKind::Dir, _) | (_, None) => (location.path.clone(), ".".to_string()),
        (_, Some(parent)) => (parent, path::file_name(&location.path).to_string()),
    };
    vpsfiles_git::log(runner(&state, &location.source).as_ref(), &cwd, &spec, limit).await
}
