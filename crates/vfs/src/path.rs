//! Path format shared by all backends and the UI.
//!
//! * Always absolute, always `/`-separated, no trailing slash except roots.
//! * Remote (SFTP) and local Unix paths are plain POSIX: `/home/me`.
//! * Local Windows paths use drive form with forward slashes: `C:/Users/me`.
//!   The drive root is `C:/`, and `/` is a virtual "This PC" root whose
//!   listing is the available drives.

use crate::{Result, VfsError};

fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// `C:/` style drive root.
pub fn is_drive_root(path: &str) -> bool {
    path.len() == 3 && is_drive(&path[..2]) && path.ends_with('/')
}

pub fn is_root(path: &str) -> bool {
    path == "/" || is_drive_root(path)
}

/// Last path component; empty for `/`, `C:` for `C:/`.
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or("")
}

/// Parent directory, or `None` for `/`.
pub fn parent(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    if is_drive(trimmed) {
        return Some("/".into());
    }
    match trimmed.rfind('/') {
        Some(0) | None => Some("/".into()),
        Some(i) => {
            let p = &trimmed[..i];
            Some(if is_drive(p) { format!("{p}/") } else { p.to_string() })
        }
    }
}

/// Append one validated name to a directory path.
pub fn join(dir: &str, name: &str) -> Result<String> {
    validate_name(name)?;
    Ok(if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") })
}

/// A single path component a user may type (new folder, rename).
pub fn validate_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\0')
        || (cfg!(windows) && name.contains(['\\', ':', '*', '?', '"', '<', '>', '|']));
    if bad { Err(VfsError::InvalidName(name.to_string())) } else { Ok(()) }
}

/// Split a name into stem and extension: `a.tar.gz` -> (`a.tar`, `gz`).
/// Dotfiles have no extension: `.bashrc` -> (`.bashrc`, ``).
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(0) | None => (name, ""),
        Some(i) => (&name[..i], &name[i + 1..]),
    }
}

/// Finder-style "keep both" name: `report.pdf` -> `report 2.pdf`.
pub fn numbered_name(name: &str, n: u32) -> String {
    let (stem, ext) = split_ext(name);
    if ext.is_empty() { format!("{stem} {n}") } else { format!("{stem} {n}.{ext}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parents() {
        assert_eq!(parent("/"), None);
        assert_eq!(parent("/home").as_deref(), Some("/"));
        assert_eq!(parent("/home/me/").as_deref(), Some("/home"));
        assert_eq!(parent("C:/").as_deref(), Some("/"));
        assert_eq!(parent("C:/Users").as_deref(), Some("C:/"));
        assert_eq!(parent("C:/Users/me").as_deref(), Some("C:/Users"));
    }

    #[test]
    fn names() {
        assert_eq!(file_name("/home/me"), "me");
        assert_eq!(file_name("/"), "");
        assert_eq!(file_name("C:/"), "C:");
        assert_eq!(join("/", "etc").unwrap(), "/etc");
        assert_eq!(join("C:/", "Users").unwrap(), "C:/Users");
        assert_eq!(join("/home", "me").unwrap(), "/home/me");
        assert!(join("/home", "../etc").is_err());
        assert!(join("/home", "a/b").is_err());
    }

    #[test]
    fn numbering() {
        assert_eq!(numbered_name("report.pdf", 2), "report 2.pdf");
        assert_eq!(numbered_name("Makefile", 3), "Makefile 3");
        assert_eq!(numbered_name(".bashrc", 2), ".bashrc 2");
        assert_eq!(numbered_name("a.tar.gz", 2), "a.tar 2.gz");
    }
}
