//! "Open in Terminal": the OS terminal in a local folder, or `ssh -t` into
//! a server folder.

use std::path::Path;
use std::process::Command;

use tauri::State;
use vpsfiles_ssh::HostOrigin;
use vpsfiles_vfs::{local, Location, Result, SourceId, VfsError};

use crate::state::AppState;

/// How a terminal emulator is told to run a command.
#[derive(Clone, Copy)]
enum Exec {
    /// `term -- cmd args…`
    DoubleDash,
    /// `term -e cmd args…`
    DashE,
    /// `term -x cmd args…`
    DashX,
    /// `term cmd args…`
    Positional,
    /// `wezterm start -- cmd args…`
    WezTerm,
}

/// Preference order; GNOME's current default (Ptyxis) first.
const LINUX_TERMINALS: &[(&str, Exec)] = &[
    ("ptyxis", Exec::DoubleDash),
    ("kgx", Exec::DoubleDash),
    ("gnome-terminal", Exec::DoubleDash),
    ("konsole", Exec::DashE),
    ("xfce4-terminal", Exec::DashX),
    ("tilix", Exec::DashE),
    ("alacritty", Exec::DashE),
    ("kitty", Exec::Positional),
    ("wezterm", Exec::WezTerm),
    ("foot", Exec::Positional),
    ("x-terminal-emulator", Exec::DashE),
    ("xterm", Exec::DashE),
];

fn which(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `ssh` arguments that reach `alias`: config hosts by alias (so ProxyJump
/// etc. apply), app hosts spelled out.
fn ssh_target(state: &AppState, alias: &str) -> Result<Vec<String>> {
    let host = state
        .ssh
        .hosts()?
        .into_iter()
        .find(|h| h.alias == alias)
        .ok_or_else(|| VfsError::NotFound(alias.to_string()))?;
    if host.origin == HostOrigin::SshConfig {
        return Ok(vec![alias.to_string()]);
    }
    let mut args = vec!["-p".to_string(), host.port.to_string()];
    for key in &host.identity_files {
        args.push("-i".into());
        args.push(key.display().to_string());
    }
    args.push(match &host.user {
        Some(u) => format!("{u}@{}", host.hostname),
        None => host.hostname.clone(),
    });
    Ok(args)
}

fn spawn(mut cmd: Command) -> Result<()> {
    let mut child = cmd.spawn().map_err(|e| VfsError::Other(format!("terminal: {e}")))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[tauri::command]
pub fn open_terminal(state: State<'_, AppState>, location: Location) -> Result<()> {
    // For a server: the command the terminal runs; for local: none.
    let remote: Option<Vec<String>> = match &location.source {
        SourceId::Local => None,
        SourceId::Ssh { host } => {
            let mut argv = vec!["ssh".to_string(), "-t".to_string()];
            argv.extend(ssh_target(&state, host)?);
            argv.push(format!("cd {} && exec \"$SHELL\" -l", quote(&location.path)));
            Some(argv)
        }
    };
    let dir = match &location.source {
        SourceId::Local => local::to_native(&location.path),
        SourceId::Ssh { .. } => dirs::home_dir().unwrap_or_else(|| Path::new("/").to_path_buf()),
    };

    if cfg!(windows) {
        let mut cmd = if which("wt.exe").is_some() {
            let mut c = Command::new("wt.exe");
            c.arg("-d").arg(&dir);
            if let Some(argv) = &remote {
                c.args(argv);
            }
            c
        } else {
            let mut c = Command::new("cmd");
            c.args(["/C", "start", ""]);
            match &remote {
                Some(argv) => c.args(argv),
                None => c.arg("cmd"),
            };
            c.current_dir(&dir);
            c
        };
        cmd.current_dir(&dir);
        return spawn(cmd);
    }

    let preferred = std::env::var("TERMINAL").ok();
    let found = preferred
        .as_deref()
        .and_then(|t| which(t).map(|p| (p, Exec::DashE)))
        .or_else(|| LINUX_TERMINALS.iter().find_map(|(name, how)| which(name).map(|p| (p, *how))))
        .ok_or_else(|| VfsError::Unsupported("no terminal emulator found".into()))?;
    let (exe, how) = found;
    let mut cmd = Command::new(&exe);
    cmd.current_dir(&dir);
    if let Some(argv) = remote {
        match how {
            Exec::DoubleDash => cmd.arg("--"),
            Exec::DashE => cmd.arg("-e"),
            Exec::DashX => cmd.arg("-x"),
            Exec::Positional => &mut cmd,
            Exec::WezTerm => cmd.args(["start", "--"]),
        };
        cmd.args(argv);
    }
    spawn(cmd)
}
