//! Read-only git state for a folder: branch, ahead/behind, changed files,
//! history. Runs the `git` CLI — locally as a process, on servers through
//! one SSH exec — so it works wherever git is installed and adds nothing to
//! the binary.
//!
//! Every invocation sets `GIT_OPTIONAL_LOCKS=0` so `git status` never takes
//! the index lock (it must not interfere with other git users on a server).

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use vpsfiles_vfs::{Result, VfsError};

/// A slow `git status` (huge repo, cold disk) shouldn't hang the UI.
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GitFileStatus {
    // Declared weakest → strongest: a folder shows its strongest child.
    Untracked,
    Added,
    Renamed,
    Modified,
    Deleted,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub root: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub remote_url: Option<String>,
    pub web_url: Option<String>,
    pub changed: u32,
    pub entries: HashMap<String, GitFileStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub date: i64,
    pub subject: String,
    pub refs: Vec<String>,
}

/// Exit code and stdout of one `git` invocation.
pub type Output = (i32, Vec<u8>);

/// Runs `git -C <cwd> <args>` for each command, in order.
/// `Ok(None)` = git is not installed there.
#[async_trait]
pub trait GitExec: Send + Sync {
    async fn run(&self, cwd: &str, cmds: &[Vec<String>]) -> Result<Option<Vec<Output>>>;
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Git state of the repo containing `dir`, or `None` outside a repo.
pub async fn info(exec: &dyn GitExec, dir: &str) -> Result<Option<GitInfo>> {
    let cmds = [
        args(&["rev-parse", "--show-toplevel", "--show-prefix"]),
        args(&["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=normal"]),
        args(&["remote", "get-url", "origin"]),
    ];
    let Some(out) = with_timeout(exec.run(dir, &cmds)).await? else { return Ok(None) };
    let [(rc, top), (sc, status), (uc, url)] = <[Output; 3]>::try_from(out)
        .map_err(|_| VfsError::Other("git: unexpected output".into()))?;
    if rc != 0 || sc != 0 {
        return Ok(None); // not a repository (or not readable)
    }
    let top = String::from_utf8_lossy(&top);
    let mut lines = top.lines();
    let root = lines.next().unwrap_or("").trim().to_string();
    let prefix = lines.next().unwrap_or("").trim().to_string();
    let mut info = parse_status(&status, &prefix);
    info.root = root;
    if uc == 0 {
        let raw = String::from_utf8_lossy(&url).trim().to_string();
        if !raw.is_empty() {
            info.web_url = web_url(&raw);
            info.remote_url = Some(strip_credentials(&raw));
        }
    }
    Ok(Some(info))
}

/// Newest-first commits touching `pathspec` (relative to `cwd`; "." = the
/// folder itself).
pub async fn log(exec: &dyn GitExec, cwd: &str, pathspec: &str, limit: u32) -> Result<Vec<GitCommit>> {
    let cmd = args(&[
        "log",
        &format!("-n{}", limit.clamp(1, 1000)),
        "--date=unix",
        "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%ad%x1f%D%x1f%s%x1e",
        "--",
        pathspec,
    ]);
    let Some(out) = with_timeout(exec.run(cwd, &[cmd])).await? else { return Ok(Vec::new()) };
    match out.into_iter().next() {
        Some((0, stdout)) => Ok(parse_log(&stdout)),
        _ => Ok(Vec::new()), // not a repo, or no commits yet
    }
}

async fn with_timeout<T>(f: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(TIMEOUT, f).await.map_err(|_| VfsError::Other("git took too long".into()))?
}

fn status_of(xy: &str) -> GitFileStatus {
    let has = |c: char| xy.contains(c);
    if has('D') {
        GitFileStatus::Deleted
    } else if has('A') {
        GitFileStatus::Added
    } else {
        GitFileStatus::Modified
    }
}

/// Parse `git status --porcelain=v2 --branch -z`. `prefix` is the queried
/// folder relative to the repo root (`--show-prefix`, ends with `/` or is
/// empty); entries outside it only count toward `changed`.
fn parse_status(raw: &[u8], prefix: &str) -> GitInfo {
    let mut info = GitInfo {
        root: String::new(),
        branch: None,
        head: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        remote_url: None,
        web_url: None,
        changed: 0,
        entries: HashMap::new(),
    };
    let text = String::from_utf8_lossy(raw);
    let mut records = text.split('\0').filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        if let Some(h) = rec.strip_prefix("# ") {
            let (key, val) = h.split_once(' ').unwrap_or((h, ""));
            match key {
                "branch.oid" if val != "(initial)" => info.head = Some(val.chars().take(7).collect()),
                "branch.head" if val != "(detached)" => info.branch = Some(val.to_string()),
                "branch.upstream" => info.upstream = Some(val.to_string()),
                "branch.ab" => {
                    for part in val.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            info.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            info.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        // Fields: "1 XY sub mH mI mW hH hI path", "2 … Xscore path" (+ orig
        // path as its own record), "u XY sub m1 m2 m3 mW h1 h2 h3 path",
        // "? path".
        let (status, path) = match rec.as_bytes().first() {
            Some(b'1') => match rec.splitn(9, ' ').collect::<Vec<_>>().as_slice() {
                [_, xy, .., path] => (status_of(xy), *path),
                _ => continue,
            },
            Some(b'2') => {
                records.next(); // original path
                match rec.splitn(10, ' ').collect::<Vec<_>>().as_slice() {
                    [.., path] => (GitFileStatus::Renamed, *path),
                    _ => continue,
                }
            }
            Some(b'u') => match rec.splitn(11, ' ').collect::<Vec<_>>().as_slice() {
                [.., path] => (GitFileStatus::Conflicted, *path),
                _ => continue,
            },
            Some(b'?') => (GitFileStatus::Untracked, &rec[2..]),
            _ => continue,
        };
        info.changed += 1;
        let Some(rest) = path.strip_prefix(prefix) else { continue };
        let name = rest.split('/').next().unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let slot = info.entries.entry(name.to_string()).or_insert(status);
        *slot = (*slot).max(status);
    }
    info
}

fn parse_log(raw: &[u8]) -> Vec<GitCommit> {
    String::from_utf8_lossy(raw)
        .split('\x1e')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_start_matches('\n').splitn(7, '\x1f').collect();
            let [hash, short, author, email, date, refs, subject] = f.as_slice() else { return None };
            Some(GitCommit {
                hash: hash.to_string(),
                short_hash: short.to_string(),
                author: author.to_string(),
                email: email.to_string(),
                date: date.trim().parse::<i64>().ok()? * 1000,
                subject: subject.trim_end().to_string(),
                refs: refs
                    .split(", ")
                    .filter(|r| !r.is_empty())
                    .map(|r| {
                        let r = r.strip_prefix("HEAD -> ").unwrap_or(r);
                        r.strip_prefix("tag: ").unwrap_or(r).to_string()
                    })
                    .filter(|r| r != "HEAD")
                    .collect(),
            })
        })
        .collect()
}

/// Drop `user:token@` from https remotes — tokens must never reach the UI.
fn strip_credentials(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://")
        && let Some((auth, host)) = rest.split_once('@')
        && !auth.contains('/')
        && scheme.starts_with("http")
    {
        return format!("{scheme}://{host}");
    }
    url.to_string()
}

/// `git@github.com:me/repo.git`, `ssh://git@host:22/me/repo`,
/// `https://host/me/repo.git` → `https://host/me/repo`.
fn web_url(remote: &str) -> Option<String> {
    let remote = strip_credentials(remote);
    let (host, path) = if let Some((scheme, rest)) = remote.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git") {
            return None;
        }
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?.to_string(), path.to_string())
    } else {
        // scp-like: [user@]host:path
        let (left, path) = remote.split_once(':')?;
        if left.contains('/') {
            return None; // a local path with a colon
        }
        let host = left.rsplit_once('@').map_or(left, |(_, h)| h);
        (host.to_string(), path.trim_start_matches('/').to_string())
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("https://{host}/{path}"))
}

// ---- runners ----------------------------------------------------------------

/// Runs git on this machine.
pub struct LocalGit;

#[async_trait]
impl GitExec for LocalGit {
    async fn run(&self, cwd: &str, cmds: &[Vec<String>]) -> Result<Option<Vec<Output>>> {
        let native = vpsfiles_vfs::local::to_native(cwd);
        let mut out = Vec::with_capacity(cmds.len());
        for cmd in cmds {
            let res = tokio::process::Command::new("git")
                .arg("-C")
                .arg(&native)
                .args(cmd)
                .env("GIT_OPTIONAL_LOCKS", "0")
                .env("LC_ALL", "C")
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .output()
                .await;
            match res {
                Ok(o) => out.push((o.status.code().unwrap_or(-1), o.stdout)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(VfsError::Other(format!("git: {e}"))),
            }
        }
        Ok(Some(out))
    }
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// One POSIX-sh script running every command, for a single SSH exec. Each
/// command's stdout is followed by `\x1e<token>:<exit code>\x1e`.
pub fn remote_script(cwd: &str, cmds: &[Vec<String>], token: &str) -> String {
    let mut s = String::from("command -v git >/dev/null 2>&1 || exit 127\nexport GIT_OPTIONAL_LOCKS=0 LC_ALL=C\n");
    for cmd in cmds {
        let quoted: Vec<String> = cmd.iter().map(|a| shell_quote(a)).collect();
        s.push_str(&format!(
            "git -C {} {} 2>/dev/null; printf '\\036%s:%d\\036' {} $?\n",
            shell_quote(cwd),
            quoted.join(" "),
            token
        ));
    }
    s
}

/// Run `script` with POSIX `sh` whatever the user's login shell is (bash,
/// zsh, fish…): ship it base64-encoded, so no shell ever parses it except sh.
pub fn sh_command(script: &str) -> String {
    format!("echo {} | base64 -d | sh", base64(script.as_bytes()))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Split the output of [`remote_script`]. `None` if git is missing.
pub fn parse_remote(exit: u32, stdout: &[u8], token: &str, n: usize) -> Option<Vec<Output>> {
    if exit == 127 {
        return None;
    }
    let marker = format!("\x1e{token}:");
    let mut rest = stdout;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let pos = find(rest, marker.as_bytes())?;
        let body = rest[..pos].to_vec();
        let after = &rest[pos + marker.len()..];
        let end = after.iter().position(|&b| b == 0x1e)?;
        let code = std::str::from_utf8(&after[..end]).ok()?.parse().ok()?;
        out.push((code, body));
        rest = &after[end + 1..];
    }
    Some(out)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_v2_with_prefix_and_folder_aggregation() {
        let raw = b"# branch.oid 1234567890abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/app.rs\0\
1 A. N... 000000 100644 100644 000 ccc src/new.rs\0\
2 R. N... 100644 100644 100644 ddd eee R100 src/lib/moved.rs\0src/lib/old.rs\0\
u UU N... 100644 100644 100644 100644 f1 f2 f3 src/lib/clash.rs\0\
? src/notes.txt\0? docs/\0";
        let info = parse_status(raw, "src/");
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert_eq!(info.head.as_deref(), Some("1234567"));
        assert_eq!(info.upstream.as_deref(), Some("origin/main"));
        assert_eq!((info.ahead, info.behind), (2, 1));
        assert_eq!(info.changed, 6);
        assert_eq!(info.entries["app.rs"], GitFileStatus::Modified);
        assert_eq!(info.entries["new.rs"], GitFileStatus::Added);
        assert_eq!(info.entries["lib"], GitFileStatus::Conflicted, "strongest child wins");
        assert_eq!(info.entries["notes.txt"], GitFileStatus::Untracked);
        assert!(!info.entries.contains_key("docs"), "outside the queried folder");

        let root = parse_status(raw, "");
        assert_eq!(root.entries["src"], GitFileStatus::Conflicted);
        assert_eq!(root.entries["docs"], GitFileStatus::Untracked);
    }

    #[test]
    fn detached_and_initial() {
        let info = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0", "");
        assert_eq!((info.branch, info.head), (None, None));
    }

    #[test]
    fn log_records() {
        let raw = b"abc123\x1fabc\x1fAda\x1fada@x.io\x1f1700000000\x1fHEAD -> main, origin/main, tag: v1.0\x1fFix: thing\x1e\n\
def456\x1fdef\x1fBob\x1fbob@x.io\x1f1690000000\x1f\x1fInitial commit\x1e\n";
        let log = parse_log(raw);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].refs, ["main", "origin/main", "v1.0"]);
        assert_eq!(log[0].date, 1_700_000_000_000);
        assert_eq!(log[1].subject, "Initial commit");
        assert!(log[1].refs.is_empty());
    }

    #[test]
    fn remote_urls() {
        assert_eq!(web_url("git@github.com:me/repo.git").as_deref(), Some("https://github.com/me/repo"));
        assert_eq!(web_url("ssh://git@gitlab.com:2222/grp/sub/repo.git").as_deref(), Some("https://gitlab.com/grp/sub/repo"));
        assert_eq!(
            web_url("https://x-access-token:ghp_SECRET@github.com/me/repo.git").as_deref(),
            Some("https://github.com/me/repo")
        );
        assert_eq!(strip_credentials("https://u:ghp_SECRET@github.com/me/r.git"), "https://github.com/me/r.git");
        assert_eq!(web_url("/srv/git/repo.git"), None);
    }

    #[test]
    fn remote_script_round_trip() {
        let token = "T0k";
        let script = remote_script("/srv/it's here", &[args(&["status"]), args(&["log"])], token);
        assert!(script.contains(r"'/srv/it'\''s here'"));
        let out = b"line1\nline2\x1eT0k:0\x1e\x1eT0k:128\x1e";
        let parsed = parse_remote(0, out, token, 2).unwrap();
        assert_eq!(parsed[0], (0, b"line1\nline2".to_vec()));
        assert_eq!(parsed[1], (128, Vec::new()));
        assert_eq!(parse_remote(127, b"", token, 2), None);
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn sh_command_runs_under_sh() {
        let script = remote_script("/", &[args(&["--version"])], "tok");
        let out = std::process::Command::new("sh").arg("-c").arg(sh_command(&script)).output().unwrap();
        let parsed = parse_remote(out.status.code().unwrap() as u32, &out.stdout, "tok", 1).unwrap();
        assert_eq!(parsed[0].0, 0);
        assert!(String::from_utf8_lossy(&parsed[0].1).starts_with("git version"));
    }

    #[tokio::test]
    async fn local_repo_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let git = |a: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(p)
                .args(a)
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@x")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@x")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "git {a:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::create_dir(p.join("src")).unwrap();
        std::fs::write(p.join("src/a.txt"), "1").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "first"]);
        git(&["remote", "add", "origin", "git@github.com:me/demo.git"]);
        std::fs::write(p.join("src/a.txt"), "2").unwrap();
        std::fs::write(p.join("src/b.txt"), "new").unwrap();

        let root = vpsfiles_vfs::local::from_native(p);
        let src = format!("{root}/src");
        let info = info(&LocalGit, &src).await.unwrap().expect("a repo");
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert_eq!(info.web_url.as_deref(), Some("https://github.com/me/demo"));
        assert_eq!(info.entries["a.txt"], GitFileStatus::Modified);
        assert_eq!(info.entries["b.txt"], GitFileStatus::Untracked);
        assert_eq!(info.changed, 2);

        let history = log(&LocalGit, &src, "a.txt", 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].subject, "first");

        let outside = tempfile::tempdir().unwrap();
        let none = info_outside(&vpsfiles_vfs::local::from_native(outside.path())).await;
        assert!(none.is_none());
    }

    async fn info_outside(dir: &str) -> Option<GitInfo> {
        info(&LocalGit, dir).await.unwrap()
    }
}
