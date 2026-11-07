use std::io;

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

pub type Result<T, E = VfsError> = std::result::Result<T, E>;

/// Every backend maps its native errors into these variants so the UI can
/// react by `code` (e.g. show a password prompt on `auth`) instead of parsing
/// messages.
///
/// JSON: `{"code":"not_found","message":"not found: /etc/x"}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VfsError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("not a directory: {0}")]
    NotADirectory(String),
    #[error("is a directory: {0}")]
    IsADirectory(String),
    #[error("directory not empty: {0}")]
    NotEmpty(String),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("not connected: {0}")]
    NotConnected(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("host key rejected: {0}")]
    HostKey(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("cancelled")]
    Cancelled,
    #[error("i/o error: {0}")]
    Io(String),
    #[error("{0}")]
    Other(String),
}

impl VfsError {
    pub fn code(&self) -> &'static str {
        match self {
            VfsError::NotFound(_) => "not_found",
            VfsError::PermissionDenied(_) => "permission_denied",
            VfsError::AlreadyExists(_) => "already_exists",
            VfsError::NotADirectory(_) => "not_a_directory",
            VfsError::IsADirectory(_) => "is_a_directory",
            VfsError::NotEmpty(_) => "not_empty",
            VfsError::InvalidName(_) => "invalid_name",
            VfsError::NotConnected(_) => "not_connected",
            VfsError::Auth(_) => "auth",
            VfsError::HostKey(_) => "host_key",
            VfsError::Unsupported(_) => "unsupported",
            VfsError::Cancelled => "cancelled",
            VfsError::Io(_) => "io",
            VfsError::Other(_) => "other",
        }
    }

    /// Map an `io::Error` while keeping the path that caused it.
    pub fn from_io(err: io::Error, path: &str) -> Self {
        let p = path.to_string();
        match err.kind() {
            io::ErrorKind::NotFound => VfsError::NotFound(p),
            io::ErrorKind::PermissionDenied => VfsError::PermissionDenied(p),
            io::ErrorKind::AlreadyExists => VfsError::AlreadyExists(p),
            io::ErrorKind::NotADirectory => VfsError::NotADirectory(p),
            io::ErrorKind::IsADirectory => VfsError::IsADirectory(p),
            io::ErrorKind::DirectoryNotEmpty => VfsError::NotEmpty(p),
            _ => VfsError::Io(format!("{path}: {err}")),
        }
    }
}

impl From<io::Error> for VfsError {
    fn from(err: io::Error) -> Self {
        VfsError::from_io(err, "")
    }
}

impl Serialize for VfsError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("VfsError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}
