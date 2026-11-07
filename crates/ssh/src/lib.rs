//! SSH side of the app.
//!
//! * [`config`] — hosts from `~/.ssh/config` (what the sidebar shows).
//! * [`SshManager`] — one live connection per host alias, connect on demand,
//!   status events, remote exec.
//! * [`SftpBackend`] — a host's filesystem as a [`vpsfiles_vfs::Backend`].
//! * [`Prompter`] — how the manager asks the UI for passwords and
//!   host-key confirmation.

mod auth;
pub mod config;
mod known_hosts;
mod manager;
mod prompt;
mod sftp;

pub use config::HostConfig;
pub use manager::{ConnEvent, ConnStatus, ExecOutput, SshManager, SshOptions};
pub use prompt::Prompter;
pub use sftp::SftpBackend;
