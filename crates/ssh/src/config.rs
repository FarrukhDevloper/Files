//! `~/.ssh/config` parsing.
//!
//! A small parser that follows OpenSSH's rules instead of `ssh2-config`, which
//! applies options that follow a `Match` line to the preceding `Host` block.
//!
//! Semantics (ssh_config(5)):
//! * Options before the first `Host` apply to every host. For each option the
//!   first value that applies wins; `IdentityFile` accumulates.
//! * `Host a b` lists `a` and `b`, unless the block sets `HostName`: then
//!   both name one server, listed once as `a` (`b` still connects). Patterns
//!   with `*`/`?` and negated `!x` patterns are never listed, but their
//!   options apply to matching aliases.
//! * `Match all` applies everywhere; any other `Match` block (`exec`, `user`,
//!   ...) can't be evaluated safely here and never applies.
//! * `Include` is expanded in place: relative paths live in `~/.ssh/`, globs
//!   (`*`, `?`) are sorted lexically, files that don't exist are skipped. An
//!   `Include` inside a block that doesn't apply contributes nothing, and the
//!   enclosing block's state is restored after the included file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vpsfiles_vfs::{Result, VfsError};

/// OpenSSH's limit for nested `Include`s; also stops include cycles.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Where a host is defined. Only [`HostOrigin::App`] hosts are editable in
/// the app; `~/.ssh/config` stays the user's file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostOrigin {
    #[default]
    SshConfig,
    App,
}

/// One concrete `Host <alias>` block with `Host *` defaults applied, or an
/// app-managed server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    pub alias: String,
    /// `HostName`, or the alias itself when absent.
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
    /// Expanded (`~` resolved) `IdentityFile` paths, in config order.
    pub identity_files: Vec<PathBuf>,
    pub proxy_jump: Option<String>,
    pub forward_agent: bool,
    pub origin: HostOrigin,
    /// A password or passphrase is saved in the keychain (from an index
    /// file, so listing never touches the keychain).
    pub has_saved_secret: bool,
    /// `IdentitiesOnly yes`: agent keys are used only if they match one of
    /// `identity_files`. Connection-only, not sent to the UI.
    #[serde(skip)]
    pub identities_only: bool,
    /// `ProxyCommand` (unsupported for now). Connection-only.
    #[serde(skip)]
    pub proxy_command: Option<String>,
}

/// The user's default config: `~/.ssh/config` (`%USERPROFILE%\.ssh\config`
/// on Windows). A missing file is an empty list, not an error.
pub fn default_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh").join("config"))
}

/// Hosts from the default config. Wildcard-only patterns (`Host *`,
/// `Host *.internal`) are applied as defaults but not listed.
pub fn load_hosts() -> Result<Vec<HostConfig>> {
    match default_config_path() {
        Some(p) => load_hosts_from(&p),
        None => Ok(Vec::new()),
    }
}

/// Every concrete alias in `path` (and its `Include`s), in config order.
pub fn load_hosts_from(path: &Path) -> Result<Vec<HostConfig>> {
    let ctx = Ctx::user();
    let lines = parse(path, &ctx)?;
    Ok(aliases(&lines).into_iter().map(|alias| evaluate(&lines, &alias, &ctx)).collect())
}

/// Effective settings for `alias`, as `ssh <alias>` would use them. Works for
/// aliases that aren't listed too (then everything is a default).
pub fn host_from(path: &Path, alias: &str) -> Result<HostConfig> {
    host_entry(path, alias).map(|(host, _)| host)
}

/// Like [`host_from`], plus whether the config names `alias` in a `Host`
/// line (listed or not).
pub(crate) fn host_entry(path: &Path, alias: &str) -> Result<(HostConfig, bool)> {
    if alias.is_empty() || alias.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(VfsError::InvalidName(alias.to_string()));
    }
    let ctx = Ctx::user();
    let lines = parse(path, &ctx)?;
    let named = concrete_names(&lines).iter().any(|a| a == alias);
    Ok((evaluate(&lines, alias, &ctx), named))
}

/// Every name the config gives a host (each concrete `Host` pattern), so
/// app hosts can't take one of them.
pub(crate) fn names_from(path: &Path) -> Result<Vec<String>> {
    Ok(concrete_names(&parse(path, &Ctx::user())?))
}

/// The local user name: OpenSSH's default `User`.
pub(crate) fn local_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".into())
}

// ---- parsing ---------------------------------------------------------------

/// Where `~` and relative `Include`s point. Injectable for tests.
struct Ctx {
    home: PathBuf,
    local_user: String,
}

impl Ctx {
    fn user() -> Self {
        Self { home: dirs::home_dir().unwrap_or_default(), local_user: local_user() }
    }
}

/// One significant config line, with `Include`s already expanded in place.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// `Host` patterns. `None` is a `Match` block that never applies.
    Section(Option<Vec<String>>),
    /// `key` is lowercase.
    Opt { key: String, args: Vec<String> },
    IncludeStart,
    IncludeEnd,
}

/// Parse `path` with its includes. A missing top-level file is empty.
fn parse(path: &Path, ctx: &Ctx) -> Result<Vec<Line>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(VfsError::from_io(e, &path.display().to_string())),
    };
    let mut out = Vec::new();
    parse_text(&text, ctx, 0, &mut out);
    Ok(out)
}

fn parse_text(text: &str, ctx: &Ctx, depth: usize, out: &mut Vec<Line>) {
    for raw in text.lines() {
        let Some((key, args)) = split_line(raw) else { continue };
        match key.as_str() {
            "host" => out.push(Line::Section(Some(args))),
            "match" => {
                let all = args.len() == 1 && args[0].eq_ignore_ascii_case("all");
                out.push(Line::Section(all.then(|| vec!["*".to_string()])));
            }
            "include" => {
                if depth >= MAX_INCLUDE_DEPTH {
                    tracing::warn!("ssh config: Include nested too deeply, ignoring {args:?}");
                    continue;
                }
                out.push(Line::IncludeStart);
                for arg in &args {
                    for file in include_paths(arg, ctx) {
                        match std::fs::read_to_string(&file) {
                            Ok(t) => parse_text(&t, ctx, depth + 1, out),
                            Err(e) => tracing::warn!("ssh config: skipping {}: {e}", file.display()),
                        }
                    }
                }
                out.push(Line::IncludeEnd);
            }
            _ => out.push(Line::Opt { key, args }),
        }
    }
}

/// `Key value`, `Key=value` or `Key = value` -> (lowercase key, args).
/// Args honour double/single quotes; an unquoted `#` starts a comment.
fn split_line(raw: &str) -> Option<(String, Vec<String>)> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let key = line[..end].to_ascii_lowercase();
    let mut rest = line[end..].trim_start();
    if let Some(r) = rest.strip_prefix('=') {
        rest = r.trim_start();
    }
    Some((key, split_args(rest)))
}

fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        match chars.peek() {
            None | Some('#') => break,
            _ => {}
        }
        let mut arg = String::new();
        let mut quote: Option<char> = None;
        while let Some(&c) = chars.peek() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => arg.push(c),
                None if c == '"' || c == '\'' => quote = Some(c),
                None if c.is_whitespace() => break,
                None => arg.push(c),
            }
            chars.next();
        }
        args.push(arg);
    }
    args
}

pub(crate) fn expand_tilde(p: &str, home: &Path) -> PathBuf {
    if p == "~" {
        home.to_path_buf()
    } else if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        home.join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Files matched by one `Include` argument, sorted.
fn include_paths(arg: &str, ctx: &Ctx) -> Vec<PathBuf> {
    let p = expand_tilde(arg, &ctx.home);
    let p = if p.is_absolute() { p } else { ctx.home.join(".ssh").join(p) };

    // Expand one component at a time; only components with wildcards hit
    // the filesystem.
    let mut found = vec![PathBuf::new()];
    for comp in p.components() {
        let comp = comp.as_os_str().to_string_lossy();
        if !comp.contains(['*', '?']) {
            found.iter_mut().for_each(|f| f.push(&*comp));
            continue;
        }
        let mut next = Vec::new();
        for dir in &found {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let mut names: Vec<String> = rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                // glob(3): wildcards don't match a leading dot.
                .filter(|n| (comp.starts_with('.') || !n.starts_with('.')) && wildcard(&comp, n))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|n| dir.join(n)));
        }
        found = next;
    }
    found.into_iter().filter(|f| f.is_file()).collect()
}

// ---- evaluation ------------------------------------------------------------

/// `*` and `?` glob match, ASCII case-insensitive (hostnames).
fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().map(|c| c.to_ascii_lowercase()).collect();
    let t: Vec<char> = text.chars().map(|c| c.to_ascii_lowercase()).collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
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

/// A `Host` line applies if any positive pattern matches and no negated
/// pattern does.
fn host_matches(patterns: &[String], alias: &str) -> bool {
    let mut hit = false;
    for pat in patterns {
        match pat.strip_prefix('!') {
            Some(neg) if wildcard(neg, alias) => return false,
            Some(_) => {}
            None => hit |= wildcard(pat, alias),
        }
    }
    hit
}

fn is_concrete(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.starts_with('!') && !pattern.contains(['*', '?'])
}

fn concrete_names(lines: &[Line]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in lines {
        if let Line::Section(Some(patterns)) = line {
            for p in patterns.iter().filter(|p| is_concrete(p)) {
                if seen.insert(p.clone()) {
                    out.push(p.clone());
                }
            }
        }
    }
    out
}

/// The servers to list: concrete names, but a block that sets `HostName`
/// is one machine (`Host tanla 203.0.113.7`), listed by its first name.
fn aliases(lines: &[Line]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Line::Section(Some(patterns)) = line else { continue };
        let sets_hostname = lines[i + 1..]
            .iter()
            .take_while(|l| !matches!(l, Line::Section(_)))
            .any(|l| matches!(l, Line::Opt { key, .. } if key == "hostname"));
        let concrete = patterns.iter().filter(|p| is_concrete(p));
        for p in concrete.take(if sets_hostname { 1 } else { usize::MAX }) {
            if seen.insert(p.clone()) {
                out.push(p.clone());
            }
        }
    }
    out
}

/// Raw option values for one alias, first value wins.
#[derive(Default)]
struct Opts {
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_files: Vec<String>,
    proxy_jump: Option<String>,
    proxy_command: Option<String>,
    forward_agent: Option<bool>,
    identities_only: Option<bool>,
}

fn yes(v: &str) -> bool {
    v.eq_ignore_ascii_case("yes") || v.eq_ignore_ascii_case("true")
}

impl Opts {
    fn apply(&mut self, key: &str, args: &[String]) {
        let Some(first) = args.first() else { return };
        match key {
            "hostname" => {
                self.hostname.get_or_insert_with(|| first.clone());
            }
            "user" => {
                self.user.get_or_insert_with(|| first.clone());
            }
            "port" => {
                if self.port.is_none() {
                    match first.parse() {
                        Ok(p) => self.port = Some(p),
                        Err(_) => tracing::warn!("ssh config: bad Port {first:?}"),
                    }
                }
            }
            "identityfile" => {
                if !self.identity_files.contains(first) {
                    self.identity_files.push(first.clone());
                }
            }
            "proxyjump" => {
                self.proxy_jump.get_or_insert_with(|| first.clone());
            }
            "proxycommand" => {
                self.proxy_command.get_or_insert_with(|| args.join(" "));
            }
            "forwardagent" => {
                self.forward_agent.get_or_insert_with(|| yes(first));
            }
            "identitiesonly" => {
                self.identities_only.get_or_insert_with(|| yes(first));
            }
            _ => {}
        }
    }
}

/// `%%`, `%h`, `%p`, `%r`, `%u`, `%d` as in ssh_config(5) TOKENS.
fn percent_expand(s: &str, vars: &[(char, &str)]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some(t) => match vars.iter().find(|(k, _)| *k == t) {
                Some((_, v)) => out.push_str(v),
                None => {
                    out.push('%');
                    out.push(t);
                }
            },
            None => out.push('%'),
        }
    }
    out
}

fn evaluate(lines: &[Line], alias: &str, ctx: &Ctx) -> HostConfig {
    let mut opts = Opts::default();
    let mut active = true; // options before the first `Host` apply to all
    let mut never = false; // inside an Include from a block that didn't apply
    let mut saved = Vec::new();
    for line in lines {
        match line {
            Line::Section(Some(patterns)) => active = !never && host_matches(patterns, alias),
            Line::Section(None) => active = false,
            Line::IncludeStart => {
                saved.push((active, never));
                never |= !active;
            }
            Line::IncludeEnd => (active, never) = saved.pop().unwrap_or((active, never)),
            Line::Opt { key, args } if active => opts.apply(key, args),
            Line::Opt { .. } => {}
        }
    }

    let hostname = opts
        .hostname
        .map(|h| percent_expand(&h, &[('h', alias)]))
        .unwrap_or_else(|| alias.to_string());
    let port = opts.port.unwrap_or(22);
    let remote_user = opts.user.clone().unwrap_or_else(|| ctx.local_user.clone());
    let home = ctx.home.to_string_lossy();
    let port_s = port.to_string();
    let vars = [
        ('h', hostname.as_str()),
        ('p', port_s.as_str()),
        ('r', remote_user.as_str()),
        ('u', ctx.local_user.as_str()),
        ('d', &*home),
    ];
    let identity_files = opts
        .identity_files
        .iter()
        .map(|f| expand_tilde(&percent_expand(f, &vars), &ctx.home))
        .collect();
    let none = |v: Option<String>| v.filter(|v| !v.eq_ignore_ascii_case("none"));

    HostConfig {
        alias: alias.to_string(),
        hostname,
        user: opts.user,
        port,
        identity_files,
        proxy_jump: none(opts.proxy_jump),
        forward_agent: opts.forward_agent.unwrap_or(false),
        origin: HostOrigin::SshConfig,
        has_saved_secret: false,
        identities_only: opts.identities_only.unwrap_or(false),
        proxy_command: none(opts.proxy_command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        home: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().to_path_buf();
            std::fs::create_dir_all(home.join(".ssh")).unwrap();
            Self { _dir: dir, home }
        }

        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let p = self.home.join(".ssh").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
            p
        }

        fn ctx(&self) -> Ctx {
            Ctx { home: self.home.clone(), local_user: "me".into() }
        }

        fn hosts(&self, text: &str) -> Vec<HostConfig> {
            let path = self.write("config", text);
            let ctx = self.ctx();
            let lines = parse(&path, &ctx).unwrap();
            aliases(&lines).into_iter().map(|a| evaluate(&lines, &a, &ctx)).collect()
        }

        fn host(&self, text: &str, alias: &str) -> HostConfig {
            let path = self.write("config", text);
            let ctx = self.ctx();
            evaluate(&parse(&path, &ctx).unwrap(), alias, &ctx)
        }
    }

    fn names(hosts: &[HostConfig]) -> Vec<&str> {
        hosts.iter().map(|h| h.alias.as_str()).collect()
    }

    #[test]
    fn lists_concrete_aliases_only() {
        let f = Fixture::new();
        let hosts = f.hosts("Host a b\nHost *.internal !x\n  User ops\nHost c?\nHost d\nHost a\n");
        assert_eq!(names(&hosts), ["a", "b", "d"]);
    }

    #[test]
    fn a_block_with_hostname_is_one_server() {
        let f = Fixture::new();
        let text = "Host tanla 203.0.113.7 !nope *.x\n  HostName 203.0.113.7\n  User root\n\
                    Host web1 web2\n  User deploy\n\
                    Host db db.lan\n  Include extra\n";
        f.write("extra", "  HostName 10.0.0.5\n");
        let hosts = f.hosts(text);
        assert_eq!(names(&hosts), ["tanla", "web1", "web2", "db"]);
        assert_eq!((hosts[1].hostname.as_str(), hosts[2].hostname.as_str()), ("web1", "web2"));
        // Every name still connects, with the block's settings.
        let other = f.host(text, "203.0.113.7");
        assert_eq!((other.hostname.as_str(), other.user.as_deref()), ("203.0.113.7", Some("root")));
        assert_eq!(f.host(text, "db.lan").hostname, "10.0.0.5");
        let path = f.write("config", text);
        let named = concrete_names(&parse(&path, &f.ctx()).unwrap());
        assert_eq!(named, ["tanla", "203.0.113.7", "web1", "web2", "db", "db.lan"]);
    }

    #[test]
    fn defaults() {
        let f = Fixture::new();
        let h = f.host("Host plain\n", "plain");
        assert_eq!(h.hostname, "plain");
        assert_eq!(h.port, 22);
        assert_eq!(h.user, None);
        assert!(h.identity_files.is_empty());
        assert_eq!(h.proxy_jump, None);
        assert!(!h.forward_agent);
        assert!(!h.identities_only);
    }

    #[test]
    fn first_value_wins_and_wildcards_apply() {
        let f = Fixture::new();
        let text = "\
User global
Host web
    Port 2222
Host web
    Port 3333
    HostName web.example.com
Host w* !wx
    HostName wildcard.example.com
    ForwardAgent yes
Host *
    User fallback
    Port 1
";
        let web = f.host(text, "web");
        assert_eq!(web.port, 2222);
        assert_eq!(web.hostname, "web.example.com");
        assert_eq!(web.user.as_deref(), Some("global"));
        assert!(web.forward_agent);

        let wx = f.host(text, "wx");
        assert_eq!(wx.hostname, "wx"); // negated
        assert_eq!(wx.port, 1);
    }

    #[test]
    fn syntax_variants() {
        let f = Fixture::new();
        let text = "\
# comment
HOST=q
\tHostName = q.example.com # trailing comment
\tport=2200
  IdentityFile \"~/.ssh/my key\"
  IdentitiesOnly yes
  ProxyCommand ssh -W %h:%p bastion
";
        let h = f.host(text, "q");
        assert_eq!(h.hostname, "q.example.com");
        assert_eq!(h.port, 2200);
        assert_eq!(h.identity_files, [f.home.join(".ssh/my key")]);
        assert!(h.identities_only);
        assert_eq!(h.proxy_command.as_deref(), Some("ssh -W %h:%p bastion"));
    }

    #[test]
    fn identity_files_accumulate_and_expand() {
        let f = Fixture::new();
        let text = "\
Host box
    HostName %h.example.com
    User root
    IdentityFile ~/.ssh/id_box
    IdentityFile %d/keys/%r@%h
Host *
    IdentityFile ~/.ssh/id_box
    IdentityFile ~/.ssh/id_all
";
        let h = f.host(text, "box");
        assert_eq!(h.hostname, "box.example.com");
        assert_eq!(
            h.identity_files,
            [
                f.home.join(".ssh/id_box"),
                f.home.join("keys/root@box.example.com"),
                f.home.join(".ssh/id_all"),
            ]
        );
    }

    #[test]
    fn match_blocks_never_leak_into_previous_host() {
        let f = Fixture::new();
        let text = "\
Host a
    HostName a.example.com
Match exec \"true\"
    User leaked
    ProxyJump leaked
Match all
    Port 2022
";
        let a = f.host(text, "a");
        assert_eq!(a.user, None);
        assert_eq!(a.proxy_jump, None);
        assert_eq!(a.port, 2022);
    }

    #[test]
    fn proxy_none_is_no_proxy() {
        let f = Fixture::new();
        let text = "Host j\n  ProxyJump none\nHost k\n  ProxyJump bastion\nHost *\n  ProxyJump other\n";
        assert_eq!(f.host(text, "j").proxy_jump, None);
        assert_eq!(f.host(text, "k").proxy_jump.as_deref(), Some("bastion"));
    }

    #[test]
    fn include_relative_glob_sorted_and_scoped() {
        let f = Fixture::new();
        f.write("conf.d/20-b.conf", "Host b\n    HostName b.example.com\n");
        f.write("conf.d/10-a.conf", "Host a\n    HostName a.example.com\n");
        f.write("conf.d/.hidden.conf", "Host hidden\n");
        f.write("conf.d/notes.txt", "Host txt\n");
        f.write("work.conf", "Port 7000\nHost w1\n");
        f.write("tilde.conf", "Host t\n    User tilde\n");
        f.write("inner.conf", "Host target\n    User inner\n");
        let text = "\
Include conf.d/*.conf ~/.ssh/tilde.conf missing/*.conf
Host w*
    Include work.conf
    User worker
Host nomatch
    Include inner.conf
";
        let hosts = f.hosts(text);
        assert_eq!(names(&hosts), ["a", "b", "t", "w1", "nomatch", "target"]);
        assert_eq!(hosts[0].hostname, "a.example.com");
        assert_eq!(hosts[2].user.as_deref(), Some("tilde"));

        // work.conf's top-level Port applies inside `Host w*`, and the
        // enclosing block (User worker) is restored after the Include.
        let w1 = &hosts[3];
        assert_eq!(w1.port, 7000);
        assert_eq!(w1.user.as_deref(), Some("worker"));
        let wz = f.host(text, "wz");
        assert_eq!((wz.port, wz.user.as_deref()), (7000, Some("worker")));
        assert_eq!(f.host(text, "zz").port, 22);

        // Included from a block that doesn't apply: contributes nothing,
        // even for a `Host` line inside it that would match.
        assert_eq!(hosts[5].user, None);
    }

    #[test]
    fn include_cycle_terminates() {
        let f = Fixture::new();
        f.write("loop.conf", "Include loop.conf\nHost looped\n");
        let hosts = f.hosts("Include loop.conf\n");
        assert_eq!(names(&hosts), ["looped"]);
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_hosts_from(&dir.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn host_from_rejects_bad_alias() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config");
        assert_eq!(host_from(&p, "a b").unwrap_err().code(), "invalid_name");
        assert_eq!(host_from(&p, "ok").unwrap().hostname, "ok");
    }

    #[test]
    fn wildcard_matching() {
        assert!(wildcard("*", "anything"));
        assert!(wildcard("*.example.com", "a.b.EXAMPLE.com"));
        assert!(wildcard("h?st", "host"));
        assert!(!wildcard("h?st", "hst"));
        assert!(wildcard("a*b*c", "aXXbYYc"));
        assert!(!wildcard("a*b*c", "aXXbYY"));
        assert!(wildcard("10.0.*", "10.0.1.2"));
    }
}
