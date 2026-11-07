//! Servers added in the app instead of `~/.ssh/config`, kept in
//! `<config dir>/vps-files/hosts.json`. Passwords never go in that file;
//! they live in the keychain (see `secrets`).

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vpsfiles_vfs::{Result, VfsError};

use crate::config::{self, HostConfig, HostOrigin};

const VERSION: u32 = 1;
const MAX_ALIAS_LEN: usize = 64;

/// The add/edit form for an app-managed server (`HostDraft` in the UI).
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDraft {
    /// Letters, digits, `.`, `_`, `-`.
    pub alias: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
    /// Local private key; `None` = ssh-agent / default keys.
    pub identity_file: Option<PathBuf>,
    /// Saved to the keychain when set; `None` keeps what is saved.
    pub password: Option<String>,
}

impl fmt::Debug for HostDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostDraft")
            .field("alias", &self.alias)
            .field("hostname", &self.hostname)
            .field("user", &self.user)
            .field("port", &self.port)
            .field("identity_file", &self.identity_file)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// One entry of hosts.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppHost {
    pub alias: String,
    pub hostname: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct HostsFile {
    version: u32,
    hosts: Vec<AppHost>,
}

pub(crate) fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= MAX_ALIAS_LEN
        && !alias.starts_with('-')
        && alias.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn bad_text(s: &str) -> bool {
    s.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl AppHost {
    /// Validate and normalize a draft. `home` expands a leading `~`.
    pub fn from_draft(draft: &HostDraft, home: &Path) -> Result<Self> {
        let alias = draft.alias.trim();
        if !valid_alias(alias) {
            return Err(VfsError::InvalidName(format!(
                "{alias:?}: use letters, digits, '.', '_' and '-' (at most {MAX_ALIAS_LEN})"
            )));
        }
        let hostname = draft.hostname.trim();
        // `[::1]` → `::1`: the brackets are URL syntax, not part of the name.
        let hostname = hostname.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(hostname);
        if hostname.is_empty() || bad_text(hostname) || hostname.contains(['/', '@']) {
            return Err(VfsError::InvalidName(format!("{hostname:?} is not a host name or address")));
        }
        if draft.port == 0 {
            return Err(VfsError::InvalidName("port 0".into()));
        }
        let user = draft.user.as_deref().map(str::trim).filter(|u| !u.is_empty());
        if let Some(u) = user.filter(|u| bad_text(u)) {
            return Err(VfsError::InvalidName(format!("{u:?} is not a user name")));
        }
        let identity_file = match &draft.identity_file {
            None => None,
            Some(p) if p.as_os_str().is_empty() => None,
            Some(p) => {
                let p = config::expand_tilde(&p.to_string_lossy(), home);
                if !p.is_absolute() {
                    return Err(VfsError::InvalidName(format!("{}: key file path must be absolute", p.display())));
                }
                if !p.is_file() {
                    return Err(VfsError::NotFound(format!("key file {}", p.display())));
                }
                Some(p)
            }
        };
        Ok(Self {
            alias: alias.to_string(),
            hostname: hostname.to_string(),
            user: user.map(str::to_string),
            port: draft.port,
            identity_file,
        })
    }

    pub fn config(&self) -> HostConfig {
        HostConfig {
            alias: self.alias.clone(),
            hostname: self.hostname.clone(),
            user: self.user.clone(),
            port: self.port,
            identity_files: self.identity_file.iter().cloned().collect(),
            proxy_jump: None,
            forward_agent: false,
            origin: HostOrigin::App,
            has_saved_secret: false,
            // The chosen key first: an agent full of other keys could use up
            // the server's MaxAuthTries before it gets a turn.
            identities_only: self.identity_file.is_some(),
            proxy_command: None,
        }
    }
}

/// Entries of `path`; a missing file is empty. A file we can't read is an
/// error, so a save never overwrites hosts it couldn't parse.
pub(crate) fn load(path: &Path) -> Result<Vec<AppHost>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(VfsError::from_io(e, &path.display().to_string())),
    };
    let file: HostsFile = serde_json::from_str(&text)
        .map_err(|e| VfsError::Other(format!("{} is damaged ({e}); fix or remove it", path.display())))?;
    if file.version > VERSION {
        return Err(VfsError::Unsupported(format!(
            "{} was written by a newer VPS Files (format {})",
            path.display(),
            file.version
        )));
    }
    Ok(file.hosts)
}

pub(crate) fn save(path: &Path, hosts: &[AppHost]) -> Result<()> {
    let mut json = serde_json::to_vec_pretty(&HostsFile { version: VERSION, hosts: hosts.to_vec() })
        .map_err(|e| VfsError::Other(format!("hosts.json: {e}")))?;
    json.push(b'\n');
    write_atomic(path, &json)
}

/// Replace `path` (mode 0600) atomically: temp file in the same dir + rename.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let io = |e: std::io::Error| VfsError::from_io(e, &path.display().to_string());
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let tmp = dir.join(tmp_name);
    let written = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(alias: &str) -> HostDraft {
        HostDraft {
            alias: alias.into(),
            hostname: "example.com".into(),
            user: None,
            port: 22,
            identity_file: None,
            password: None,
        }
    }

    #[test]
    fn aliases() {
        for ok in ["web", "web-1", "a.b_c", "X9"] {
            assert!(valid_alias(ok), "{ok}");
        }
        for bad in ["", "-x", "a b", "a/b", "ü", "a:b", "a*", &"x".repeat(65)] {
            assert!(!valid_alias(bad), "{bad}");
        }
    }

    #[test]
    fn drafts_are_normalized_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("key"), "k").unwrap();

        let mut d = draft(" web ");
        d.hostname = " [::1] ".into();
        d.user = Some("  ".into());
        d.identity_file = Some("~/key".into());
        let h = AppHost::from_draft(&d, home).unwrap();
        assert_eq!(h.alias, "web");
        assert_eq!(h.hostname, "::1");
        assert_eq!(h.user, None);
        assert_eq!(h.identity_file.as_deref(), Some(home.join("key").as_path()));
        let c = h.config();
        assert_eq!((c.origin, c.identities_only, c.identity_files.len()), (HostOrigin::App, true, 1));

        let err = |d: HostDraft| AppHost::from_draft(&d, home).unwrap_err().code();
        assert_eq!(err(draft("a b")), "invalid_name");
        assert_eq!(err(HostDraft { hostname: "".into(), ..draft("w") }), "invalid_name");
        assert_eq!(err(HostDraft { hostname: "u@h".into(), ..draft("w") }), "invalid_name");
        assert_eq!(err(HostDraft { port: 0, ..draft("w") }), "invalid_name");
        assert_eq!(err(HostDraft { user: Some("a b".into()), ..draft("w") }), "invalid_name");
        assert_eq!(err(HostDraft { identity_file: Some("rel/key".into()), ..draft("w") }), "invalid_name");
        assert_eq!(err(HostDraft { identity_file: Some(home.join("nope")), ..draft("w") }), "not_found");
    }

    #[test]
    fn file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/hosts.json");
        assert_eq!(load(&path).unwrap(), vec![]);
        let hosts = vec![AppHost::from_draft(&draft("a"), dir.path()).unwrap()];
        save(&path, &hosts).unwrap();
        assert_eq!(load(&path).unwrap(), hosts);
        // Only the file itself is left behind.
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(load(&path).unwrap_err().code(), "other");
        std::fs::write(&path, r#"{"version": 9, "hosts": []}"#).unwrap();
        assert_eq!(load(&path).unwrap_err().code(), "unsupported");
    }
}
