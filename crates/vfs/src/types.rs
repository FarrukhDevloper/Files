use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Which filesystem a path belongs to.
///
/// JSON: `{"kind":"local"}` or `{"kind":"ssh","host":"tanla"}`.
/// String form (URLs, map keys): `local` or `ssh:tanla`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SourceId {
    Local,
    /// `host` is the alias from `~/.ssh/config` (`Host <alias>`).
    Ssh { host: String },
}

impl SourceId {
    pub fn ssh(host: impl Into<String>) -> Self {
        SourceId::Ssh { host: host.into() }
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceId::Local => f.write_str("local"),
            SourceId::Ssh { host } => write!(f, "ssh:{host}"),
        }
    }
}

impl FromStr for SourceId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(SourceId::Local),
            _ => match s.strip_prefix("ssh:") {
                Some(host) if !host.is_empty() => Ok(SourceId::ssh(host)),
                _ => Err(format!("invalid source id: {s}")),
            },
        }
    }
}

/// A path on a specific source. See [`crate::path`] for the path format.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Location {
    pub source: SourceId,
    pub path: String,
}

impl Location {
    pub fn new(source: SourceId, path: impl Into<String>) -> Self {
        Self { source, path: path.into() }
    }

    pub fn local(path: impl Into<String>) -> Self {
        Self::new(SourceId::Local, path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    /// Sockets, devices, fifos, broken symlinks.
    Other,
}

/// One row in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    /// Absolute path in the source's path format.
    pub path: String,
    /// For symlinks this is the kind of the *target* (so the UI knows whether
    /// it can navigate into it). Broken links are `Other`.
    pub kind: EntryKind,
    pub size: u64,
    /// Unix epoch milliseconds.
    pub modified: Option<i64>,
    /// Permission bits (`st_mode & 0o7777`) when the backend knows them.
    pub mode: Option<u32>,
    pub is_symlink: bool,
    pub is_hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
    Root,
    Drive,
}

/// A sidebar shortcut on the local machine ("Favourites" / "Locations").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub label: String,
    pub path: String,
    pub kind: PlaceKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_round_trips() {
        for s in ["local", "ssh:tanla", "ssh:user@host"] {
            assert_eq!(s.parse::<SourceId>().unwrap().to_string(), s);
        }
        assert!("ssh:".parse::<SourceId>().is_err());
        assert!("ftp:x".parse::<SourceId>().is_err());
    }

    #[test]
    fn json_shapes_match_ipc_contract() {
        assert_eq!(serde_json::to_string(&SourceId::Local).unwrap(), r#"{"kind":"local"}"#);
        assert_eq!(
            serde_json::to_string(&SourceId::ssh("tanla")).unwrap(),
            r#"{"kind":"ssh","host":"tanla"}"#
        );
        assert_eq!(serde_json::to_string(&EntryKind::Dir).unwrap(), r#""dir""#);
    }
}
