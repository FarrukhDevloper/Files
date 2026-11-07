//! SSH side of the app.
//!
//! * [`config`] — hosts from `~/.ssh/config` (what the sidebar shows).
//! * [`SshManager`] — one live connection per host alias, connect on demand,
//!   status events, remote exec; app-managed hosts ([`HostDraft`]) and
//!   passwords saved in the keychain ([`SecretStore`]).
//! * [`SftpBackend`] — a host's filesystem as a [`vpsfiles_vfs::Backend`].
//! * [`Prompter`] — how the manager asks the UI for passwords and
//!   host-key confirmation.

mod auth;
pub mod config;
mod hosts;
mod info;
mod known_hosts;
mod manager;
mod prompt;
mod secrets;
mod sftp;
mod sftp_io;
mod shell;

pub use config::{HostConfig, HostOrigin};
pub use hosts::HostDraft;
pub use info::ServerInfo;
pub use manager::{ConnEvent, ConnStatus, ExecOutput, SshManager, SshOptions, SshTuning};
pub use prompt::{Prompter, Secret};
pub use secrets::{OsKeychain, SecretStore};
pub use sftp::SftpBackend;
