//! `~/.ssh/known_hosts` lookup and TOFU append, OpenSSH semantics.
//!
//! Lines are parsed one by one with `ssh_key`'s known_hosts parser (after
//! collapsing whitespace), so one malformed or unsupported line never breaks
//! the whole file. Supports hashed names (`|1|salt|hash`), `[host]:port`,
//! `*`/`?` patterns, `!negation` and `@revoked`. `@cert-authority` lines
//! are ignored: we never ask servers for host certificates.

use std::io::Write;
use std::path::Path;

use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::known_hosts::{Entry, HostPatterns, Marker};
use russh::keys::{Algorithm, PublicKey};
use sha1::Sha1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Check {
    /// A matching line has exactly this key.
    Known,
    /// No line for this host has a key of this type.
    Unknown,
    /// A line for this host has a *different* key of the same type.
    Changed,
    /// The key is marked `@revoked`.
    Revoked,
}

/// The name OpenSSH looks up: `host`, or `[host]:port` off port 22.
fn lookup_name(host: &str, port: u16) -> String {
    if port == 22 { host.to_string() } else { format!("[{host}]:{port}") }
}

fn entries(path: &Path) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .filter_map(|line| {
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            match line.parse::<Entry>() {
                Ok(e) => Some(e),
                Err(e) => {
                    tracing::debug!("known_hosts: skipping unparsable line: {e}");
                    None
                }
            }
        })
        .collect()
}

fn wildcard(pattern: &str, text: &str) -> bool {
    // Same matcher as ssh_config; hostnames are case-insensitive.
    let p: Vec<char> = pattern.chars().map(|c| c.to_ascii_lowercase()).collect();
    let t: Vec<char> = text.chars().map(|c| c.to_ascii_lowercase()).collect();
    let (mut pi, mut ti, mut star) = (0, 0, None);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn host_matches(patterns: &HostPatterns, name: &str) -> bool {
    match patterns {
        HostPatterns::HashedName { salt, hash } => {
            let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(salt) else { return false };
            mac.update(name.as_bytes());
            mac.verify_slice(hash).is_ok()
        }
        HostPatterns::Patterns(list) => {
            let mut hit = false;
            for p in list {
                match p.strip_prefix('!') {
                    Some(neg) if wildcard(neg, name) => return false,
                    Some(_) => {}
                    None => hit |= wildcard(p, name),
                }
            }
            hit
        }
    }
}

/// RSA keys are one type whatever signature hash gets negotiated.
pub(crate) fn same_type(a: &Algorithm, b: &Algorithm) -> bool {
    let rsa = |x: &Algorithm| matches!(x, Algorithm::Rsa { .. });
    (rsa(a) && rsa(b)) || a == b
}

pub(crate) fn check(path: &Path, host: &str, port: u16, key: &PublicKey) -> Check {
    let name = lookup_name(host, port);
    let mut changed = false;
    let mut known = false;
    for e in entries(path).iter().filter(|e| host_matches(e.host_patterns(), &name)) {
        let same_key = e.public_key().key_data() == key.key_data();
        match e.marker() {
            Some(Marker::Revoked) if same_key => return Check::Revoked,
            Some(_) => {}
            None if same_key => known = true,
            None => changed |= same_type(&e.public_key().algorithm(), &key.algorithm()),
        }
    }
    if known {
        Check::Known
    } else if changed {
        Check::Changed
    } else {
        Check::Unknown
    }
}

/// Key types already recorded for this host. The client prefers these during
/// key exchange, like OpenSSH, so a server with several host keys presents
/// the one we can verify instead of triggering a needless TOFU prompt.
pub(crate) fn known_algorithms(path: &Path, host: &str, port: u16) -> Vec<Algorithm> {
    let name = lookup_name(host, port);
    let mut out: Vec<Algorithm> = Vec::new();
    for e in entries(path) {
        if e.marker().is_none() && host_matches(e.host_patterns(), &name) {
            let alg = e.public_key().algorithm();
            if !out.iter().any(|a| same_type(a, &alg)) {
                out.push(alg);
            }
        }
    }
    out
}

/// Append a trusted key. Follows the file's convention: if it already holds
/// hashed names (`HashKnownHosts yes`), the new name is hashed too.
pub(crate) fn append(path: &Path, host: &str, port: u16, key: &PublicKey) -> std::io::Result<()> {
    let name = lookup_name(host, port);
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let hashed = existing.lines().any(|l| l.trim_start().starts_with("|1|"));
    let host_field = if hashed { hash_name(&name).to_string() } else { name };
    let key_str = key.to_openssh().map_err(std::io::Error::other)?;
    // Drop the key comment; known_hosts lines are `host type base64`.
    let key_str = key_str.split(' ').take(2).collect::<Vec<_>>().join(" ");

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let sep = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    writeln!(f, "{sep}{host_field} {key_str}")
}

fn hash_name(name: &str) -> HostPatterns {
    // 20 random bytes like OpenSSH; uuid v4 draws from the OS RNG.
    let mut salt = Vec::with_capacity(32);
    salt.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    salt.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    salt.truncate(20);
    let mut mac = Hmac::<Sha1>::new_from_slice(&salt).expect("hmac accepts any key length");
    mac.update(name.as_bytes());
    let hash: [u8; 20] = mac.finalize().into_bytes().into();
    HostPatterns::HashedName { salt, hash }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::private::Ed25519Keypair;
    use russh::keys::PrivateKey;

    fn key(seed: u8) -> PublicKey {
        PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32])).public_key().clone()
    }

    fn line(host: &str, k: &PublicKey) -> String {
        let s = k.to_openssh().unwrap();
        format!("{host} {s}\n")
    }

    fn file(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("known_hosts");
        std::fs::write(&p, text).unwrap();
        (dir, p)
    }

    #[test]
    fn plain_ports_and_patterns() {
        let (a, b) = (key(1), key(2));
        let text = format!(
            "# comment\n\ngarbage line\n{}{}{}{}",
            line("example.com,10.0.0.1", &a),
            line("[example.com]:2222", &b),
            line("*.wild.test,!bad.wild.test", &a),
            line("tab.test", &a).replace(' ', "\t  "),
        );
        let (_d, p) = file(&text);
        assert_eq!(check(&p, "example.com", 22, &a), Check::Known);
        assert_eq!(check(&p, "EXAMPLE.com", 22, &a), Check::Known);
        assert_eq!(check(&p, "10.0.0.1", 22, &a), Check::Known);
        assert_eq!(check(&p, "example.com", 22, &b), Check::Changed);
        assert_eq!(check(&p, "example.com", 2222, &b), Check::Known);
        assert_eq!(check(&p, "example.com", 2222, &a), Check::Changed);
        assert_eq!(check(&p, "example.com", 2223, &a), Check::Unknown);
        assert_eq!(check(&p, "x.wild.test", 22, &a), Check::Known);
        assert_eq!(check(&p, "bad.wild.test", 22, &a), Check::Unknown);
        assert_eq!(check(&p, "tab.test", 22, &a), Check::Known);
        assert_eq!(check(&p, "other.test", 22, &a), Check::Unknown);
    }

    #[test]
    fn hashed_entries() {
        let a = key(3);
        let hashed = hash_name("[127.0.0.1]:2200").to_string();
        let (_d, p) = file(&line(&hashed, &a));
        assert_eq!(check(&p, "127.0.0.1", 2200, &a), Check::Known);
        assert_eq!(check(&p, "127.0.0.1", 22, &a), Check::Unknown);
        assert_eq!(check(&p, "127.0.0.1", 2200, &key(4)), Check::Changed);
    }

    #[test]
    fn openssh_hashed_vector() {
        // From russh's tests: hashes "example.com" with a known salt.
        let k = russh::keys::parse_public_key_base64(
            "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF",
        )
        .unwrap();
        let (_d, p) = file(&line("|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak=", &k));
        assert_eq!(check(&p, "example.com", 22, &k), Check::Known);
    }

    #[test]
    fn revoked_and_other_types() {
        let a = key(5);
        let text = format!("@revoked {}", line("host.test", &a));
        let (_d, p) = file(&text);
        assert_eq!(check(&p, "host.test", 22, &a), Check::Revoked);

        // Only a different key type known -> Unknown (prompt), like OpenSSH.
        // A truncated key on the same host must not break the file either.
        let rsa = "host2.test ssh-ed25519 AAAAC3Nza\nhost2.test ssh-rsa \
            AAAAB3NzaC1yc2EAAAADAQABAAAAgQDUBvU/qPauO5U+gLiqOltMYmNIbg1Xz4mZQW42SG6OFkjzcqmRC6Q5AI\
            WP82LmrNiui3JfD58F94mVJmlw9166GOTRlocEOWyMi+2K3rtolvZ8cIqgpGyTH5lNm9DilvwIzd+D2nFfWZ3F\
            WLyHs9RKe3VY2lXuWDvP2ZrPEmeFJw==\n";
        let (_d2, p2) = file(rsa);
        assert_eq!(check(&p2, "host2.test", 22, &a), Check::Unknown);
        assert_eq!(known_algorithms(&p2, "host2.test", 22).len(), 1);
        assert!(matches!(known_algorithms(&p2, "host2.test", 22)[0], Algorithm::Rsa { .. }));
    }

    #[test]
    fn append_follows_file_convention() {
        let a = key(6);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/known_hosts");
        append(&p, "plain.test", 2022, &a).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("[plain.test]:2022 ssh-ed25519 "), "{text}");
        assert_eq!(check(&p, "plain.test", 2022, &a), Check::Known);
        assert_eq!(known_algorithms(&p, "plain.test", 2022), [Algorithm::Ed25519]);

        let hashed = hash_name("seed.test").to_string();
        let (_d, p) = file(line(&hashed, &key(7)).trim_end()); // no final newline
        append(&p, "new.test", 22, &a).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().all(|l| l.starts_with("|1|")), "{text}");
        assert_eq!(check(&p, "new.test", 22, &a), Check::Known);
    }
}
