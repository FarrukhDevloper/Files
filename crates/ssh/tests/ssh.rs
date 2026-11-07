//! End-to-end tests against the in-process server in `support/`. Fully
//! hermetic: 127.0.0.1, temp dirs, no real `~/.ssh`.

#![cfg(unix)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use russh::keys::ssh_key::LineEnding;
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde_json::json;
use vpsfiles_ssh::{ConnStatus, HostDraft, HostOrigin, SshManager, SshOptions};
use vpsfiles_vfs::{path, EntryKind, VfsError, WriteMode};

/// ed25519, passphrase "hunter2" (throwaway, generated for this test).
const ENCRYPTED_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABCBcD/tCd
5YBjHi5U4FeQOQAAAAGAAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIKZWkiIZH8KvyZ4V
4u2M4roPa1q5earjSlCeABoIQGbZAAAAkCMYfB3k6qMVJSEZFuAB/RmkcVSI5FhtnXe5iT
UnEDAgcb3PTyBurHcL1XemoV90/SEFHdnwR4P4/5Hz1u50wwVHOH98kaGT2v1RiSEys97e
BHBMyBWxNRGYin/sWi0TuuNhAaMlp/iBL1Sf/N/K4jg9kuwh6K6t83bcvcy3ZXpNGQ/WAK
R55eO+6EZ/BGLgnQ==
-----END OPENSSH PRIVATE KEY-----
";

fn code<T>(r: Result<T, VfsError>) -> &'static str {
    match r {
        Ok(_) => "ok",
        Err(e) => e.code(),
    }
}

fn tmp() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().to_string_lossy().into_owned();
    (d, p)
}

// ---- host keys ------------------------------------------------------------

#[tokio::test]
async fn host_key_tofu_then_known_then_changed() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    home.config(server.port, "");
    let prompter = || Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..TestPrompter::trusting() });

    // First contact: asked once, remembered as [127.0.0.1]:port.
    let p1 = prompter();
    home.manager(p1.clone()).backend("test").await.unwrap();
    assert_eq!(p1.asked().0, 1);
    let kh = home.known_hosts();
    assert!(kh.starts_with(&format!("[127.0.0.1]:{} ssh-ed25519 ", server.port)), "{kh}");

    // Known now: no question.
    let p2 = prompter();
    home.manager(p2.clone()).backend("test").await.unwrap();
    assert_eq!(p2.asked().0, 0);

    // Key changed: hard fail, never prompted, file untouched.
    server.set_host_key(key(2));
    let p3 = prompter();
    let mgr = home.manager(p3.clone());
    assert_eq!(code(mgr.backend("test").await), "host_key");
    assert_eq!(p3.asked(), (0, 0, 0));
    assert!(matches!(mgr.status("test"), ConnStatus::Failed { error: VfsError::HostKey(_) }));
    assert_eq!(home.known_hosts(), kh);
}

#[tokio::test]
async fn declined_host_key_is_cancel_and_not_saved() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    home.config(server.port, "");
    let p = Arc::new(TestPrompter::default()); // declines
    let mgr = home.manager(p.clone());
    assert_eq!(code(mgr.backend("test").await), "cancelled");
    assert_eq!(p.asked().0, 1);
    assert_eq!(home.known_hosts(), "");
    assert_eq!(mgr.status("test"), ConnStatus::Disconnected);
}

#[tokio::test]
async fn hashed_known_hosts_are_followed() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    home.config(server.port, "");
    // Pre-existing hashed entry for some other host: new entries get hashed.
    home.write(
        "known_hosts",
        "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= ssh-ed25519 \
         AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF\n",
    );
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..TestPrompter::trusting() });
    home.manager(p.clone()).backend("test").await.unwrap();
    let kh = home.known_hosts();
    assert_eq!(kh.lines().count(), 2);
    assert!(kh.lines().all(|l| l.starts_with("|1|")), "{kh}");
    assert!(!kh.contains("127.0.0.1"));

    let p2 = Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..Default::default() });
    home.manager(p2.clone()).backend("test").await.unwrap();
    assert_eq!(p2.asked().0, 0);
}

// ---- auth -----------------------------------------------------------------

#[tokio::test]
async fn password_auth_and_failures() {
    let (_server, home, p, mgr) = connected().await;
    mgr.backend("test").await.unwrap();
    assert_eq!(p.asked(), (1, 1, 0));

    // Wrong password: three tries, then `auth`.
    let wrong = Arc::new(TestPrompter { password: Some("nope".into()), ..TestPrompter::trusting() });
    let mgr = home.manager(wrong.clone());
    assert_eq!(code(mgr.backend("test").await), "auth");
    assert_eq!(wrong.asked().1, 3);
    assert!(matches!(mgr.status("test"), ConnStatus::Failed { error: VfsError::Auth(_) }));

    // Cancelled password prompt.
    let cancel = Arc::new(TestPrompter::trusting());
    assert_eq!(code(home.manager(cancel).backend("test").await), "cancelled");
}

#[tokio::test]
async fn key_file_auth_with_and_without_passphrase() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();

    // Plain key as the default identity (no IdentityFile in config).
    let plain = key(9);
    home.write("id_ed25519", &plain.to_openssh(LineEnding::LF).unwrap());
    server.authorize(plain.public_key());
    home.config(server.port, "");
    let p = Arc::new(TestPrompter::trusting());
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (1, 0, 0));

    // Encrypted key from IdentityFile: passphrase asked once.
    let enc = home.write("enc_key", ENCRYPTED_KEY);
    let enc_pub = russh::keys::ssh_key::PrivateKey::from_openssh(ENCRYPTED_KEY).unwrap().public_key().clone();
    server.authorize(&enc_pub);
    home.config(server.port, &format!("  IdentityFile {}\n", enc.display()));
    let p = Arc::new(TestPrompter { passphrase: Some("hunter2".into()), ..TestPrompter::trusting() });
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 0, 1));

    // Wrong passphrase: the key is skipped after 3 tries, password is next.
    let p = Arc::new(TestPrompter {
        passphrase: Some("wrong".into()),
        password: Some(PASSWORD.into()),
        ..TestPrompter::trusting()
    });
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 1, 3));
}

// ---- keychain -------------------------------------------------------------

#[tokio::test]
async fn remembered_password_is_used_and_replaced_when_wrong() {
    let (_server, home, _p, _mgr) = connected().await;
    let saved = |m: &SshManager| m.hosts().unwrap()[0].has_saved_secret;
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), remember: true, ..TestPrompter::trusting() });
    let mgr = home.manager(p.clone());
    assert!(!saved(&mgr));
    mgr.backend("test").await.unwrap();
    assert_eq!(p.asked(), (1, 1, 0));
    assert_eq!(home.secrets.item("test"), Some(json!({ "password": PASSWORD })));
    assert!(saved(&mgr));
    let json = serde_json::to_value(mgr.hosts().unwrap()).unwrap();
    assert_eq!(json[0]["hasSavedSecret"], true);

    // Next start: nothing to ask.
    let p = Arc::new(TestPrompter::default());
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 0, 0));

    // A stale saved password is forgotten and the user asked; without
    // "remember" the new one isn't saved.
    home.secrets.put("test", json!({ "password": "old" }));
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..Default::default() });
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 1, 0));
    assert_eq!(home.secrets.item("test"), None);
    assert!(!saved(&mgr));

    // Listing reads only the index. If the keychain lost the item behind
    // our back, the next connect corrects the index.
    home.secrets.put("test", json!({ "password": PASSWORD }));
    mgr.disconnect("test").await;
    mgr.backend("test").await.unwrap();
    mgr.disconnect("test").await;
    assert!(saved(&mgr));
    home.secrets.items.lock().unwrap().clear();
    let calls = home.secrets.calls.load(std::sync::atomic::Ordering::SeqCst);
    assert!(saved(&mgr), "stale until the keychain is read");
    assert_eq!(home.secrets.calls.load(std::sync::atomic::Ordering::SeqCst), calls, "listing never opens it");
    let no_password = home.manager(Arc::new(TestPrompter::default()));
    assert_eq!(code(no_password.backend("test").await), "cancelled");
    assert!(!saved(&mgr));
}

#[tokio::test]
async fn remembered_passphrase_skips_the_prompt() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    let enc = home.write("enc_key", ENCRYPTED_KEY);
    let enc_pub = russh::keys::ssh_key::PrivateKey::from_openssh(ENCRYPTED_KEY).unwrap().public_key().clone();
    server.authorize(&enc_pub);
    home.config(server.port, &format!("  IdentityFile {}\n", enc.display()));
    let enc = enc.display().to_string();

    let p = Arc::new(TestPrompter { passphrase: Some("hunter2".into()), remember: true, ..TestPrompter::trusting() });
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (1, 0, 1));
    assert_eq!(home.secrets.item("test"), Some(json!({ "passphrases": { &enc: "hunter2" } })));

    let p = Arc::new(TestPrompter::default());
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 0, 0));

    // Wrong saved passphrase: dropped, asked, saved again (remember).
    home.secrets.put("test", json!({ "password": "kept", "passphrases": { &enc: "nope" } }));
    let p = Arc::new(TestPrompter { passphrase: Some("hunter2".into()), remember: true, ..Default::default() });
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 0, 1));
    assert_eq!(home.secrets.item("test"), Some(json!({ "password": "kept", "passphrases": { &enc: "hunter2" } })));
}

#[tokio::test]
async fn broken_keychain_never_blocks_connecting() {
    let (_server, home, _p, _mgr) = connected().await;
    home.secrets.broken.store(true, std::sync::atomic::Ordering::SeqCst);
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), remember: true, ..TestPrompter::trusting() });
    let mgr = home.manager(p.clone());
    mgr.backend("test").await.unwrap();
    assert_eq!(p.asked(), (1, 1, 0));

    // Remembered for this session only.
    mgr.disconnect("test").await;
    mgr.backend("test").await.unwrap();
    assert_eq!(p.asked(), (1, 1, 0));
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..Default::default() });
    let mgr = home.manager(p.clone());
    mgr.backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 1, 0));
    assert!(home.secrets.items.lock().unwrap().is_empty());
    assert!(!mgr.hosts().unwrap()[0].has_saved_secret, "session-only secrets aren't 'saved'");
}

/// A stream of accepted Unix connections for russh's agent server.
fn unix_incoming(
    l: tokio::net::UnixListener,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = std::io::Result<tokio::net::UnixStream>> + Send>> {
    Box::pin(futures::stream::unfold(l, |l| async move {
        let r = l.accept().await.map(|(s, _)| s);
        Some((r, l))
    }))
}

#[tokio::test]
async fn agent_auth_respects_identities_only() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    let sock = home.path("agent.sock");
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    tokio::spawn(russh::keys::agent::server::serve(unix_incoming(listener), ()));

    let agent_key = key(11);
    let mut client = russh::keys::agent::client::AgentClient::connect_uds(&sock).await.unwrap();
    client.add_identity(&agent_key, &[]).await.unwrap();
    server.authorize(agent_key.public_key());

    let mut opts = home.options();
    opts.agent = Some(sock.clone());
    home.config(server.port, "");
    let p = Arc::new(TestPrompter::trusting());
    SshManager::with_options(p.clone(), opts.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (1, 0, 0), "agent key used, nothing else asked");

    // IdentitiesOnly with a different configured key: the agent key is not
    // offered, so we end up at the password.
    let other = key(12);
    let other_path = home.write("other_key", &other.to_openssh(LineEnding::LF).unwrap());
    home.config(
        server.port,
        &format!("  IdentityFile {}\n  IdentitiesOnly yes\n", other_path.display()),
    );
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..Default::default() });
    SshManager::with_options(p.clone(), opts).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 1, 0));
}

// ---- manager --------------------------------------------------------------

#[tokio::test]
async fn concurrent_callers_share_one_attempt() {
    let (server, _home, p, mgr) = connected().await;
    let mgr = Arc::new(mgr);
    let calls: Vec<_> = (0..8)
        .map(|_| {
            let m = mgr.clone();
            tokio::spawn(async move { m.backend("test").await.map(|_| ()) })
        })
        .collect();
    for c in calls {
        c.await.unwrap().unwrap();
    }
    // One TCP connection for the TOFU check + one real one.
    assert_eq!(server.count(|c| &c.connections), 2);
    assert_eq!(p.asked(), (1, 1, 0));
    mgr.backend("test").await.unwrap();
    assert_eq!(server.count(|c| &c.connections), 2);
}

async fn next_status(rx: &mut tokio::sync::broadcast::Receiver<vpsfiles_ssh::ConnEvent>) -> ConnStatus {
    let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("no event").unwrap();
    assert_eq!(ev.host, "test");
    ev.status
}

#[tokio::test]
async fn status_events_drop_and_reconnect() {
    let (server, _home, _p, mgr) = connected().await;
    let mut rx = mgr.subscribe();
    assert_eq!(mgr.status("test"), ConnStatus::Disconnected);

    mgr.backend("test").await.unwrap();
    assert_eq!(next_status(&mut rx).await, ConnStatus::Connecting);
    assert_eq!(next_status(&mut rx).await, ConnStatus::Connected);
    assert_eq!(mgr.status("test"), ConnStatus::Connected);

    // Server goes away: status flips by itself, the next call reconnects.
    server.drop_sessions();
    assert_eq!(next_status(&mut rx).await, ConnStatus::Disconnected);
    assert_eq!(mgr.status("test"), ConnStatus::Disconnected);
    let before = server.count(|c| &c.connections);
    let fs = mgr.backend("test").await.unwrap();
    assert_eq!(server.count(|c| &c.connections), before + 1);
    fs.home().await.unwrap();
    assert_eq!(next_status(&mut rx).await, ConnStatus::Connecting);
    assert_eq!(next_status(&mut rx).await, ConnStatus::Connected);

    // Explicit disconnect: clean close, old backend reports not_connected.
    mgr.disconnect("test").await;
    assert_eq!(next_status(&mut rx).await, ConnStatus::Disconnected);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(code(fs.home().await), "not_connected");
    assert!(rx.try_recv().is_err(), "no duplicate Disconnected event");
}

#[tokio::test]
async fn exec_collects_output_and_status() {
    let (_server, _home, _p, mgr) = connected().await;
    let out = mgr.exec("test", "printf out; printf err >&2; exit 3").await.unwrap();
    assert_eq!(out.exit_status, 3);
    assert_eq!(out.stdout, b"out");
    assert_eq!(out.stderr, b"err");
    // stdin is closed: `cat` returns at once instead of hanging.
    let out = mgr.exec("test", "cat; echo done").await.unwrap();
    assert_eq!((out.exit_status, out.stdout.as_slice()), (0, &b"done\n"[..]));
}

#[tokio::test]
async fn server_info_from_one_command() {
    // The test server runs commands locally: this machine's /proc.
    let (server, _home, _p, mgr) = connected().await;
    // Quiet: it doesn't sign in by itself...
    assert_eq!(code(mgr.server_info("test").await), "sign_in_needed");
    // ...but uses the connection the user opened.
    mgr.backend("test").await.unwrap();
    let info = mgr.server_info("test").await.unwrap();
    assert_eq!(server.count(|c| &c.execs), 1);
    assert!(info.kernel.is_some() && info.os.is_some(), "{info:?}");
    if cfg!(target_os = "linux") {
        assert!(info.cpus.unwrap() >= 1);
        assert!(info.mem_total.unwrap() >= info.mem_available.unwrap());
        assert!(info.load.is_some() && info.uptime_secs.is_some(), "{info:?}");
    }
    // No shell (sftp-only account): unsupported.
    *server.state.allow_exec.lock().unwrap() = false;
    assert_eq!(code(mgr.server_info("test").await), "unsupported");
}

#[tokio::test]
async fn hosts_come_from_configured_file() {
    let home = Home::new();
    home.write("config", "Host a b\nHost *\n  User x\n");
    let mgr = home.manager(Arc::new(TestPrompter::default()));
    let hosts = mgr.hosts().unwrap();
    assert_eq!(hosts.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(hosts[0].user.as_deref(), Some("x"));
}

// ---- quiet connects and background reconnects ----------------------------

fn sign_in_reason<T>(r: Result<T, VfsError>) -> String {
    match r {
        Err(VfsError::SignInNeeded(why)) => why,
        Err(e) => panic!("expected sign_in_needed, got {e:?}"),
        Ok(_) => panic!("expected sign_in_needed, got Ok"),
    }
}

#[tokio::test]
async fn quiet_connect_never_prompts() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    home.config(server.port, "");
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), remember: true, ..TestPrompter::trusting() });
    let mgr = home.manager(p.clone());

    // Unknown host key: not asked, not saved.
    assert!(sign_in_reason(mgr.connect("test", false).await).contains("host key"));
    assert!(matches!(mgr.status("test"), ConnStatus::Failed { error: VfsError::SignInNeeded(_) }));
    assert_eq!((p.asked(), home.known_hosts()), ((0, 0, 0), String::new()));

    // The user connects once (and saves the password)...
    mgr.connect("test", true).await.unwrap();
    assert_eq!(p.asked(), (1, 1, 0));
    mgr.disconnect("test").await;
    // ...after that quiet works: keychain, no questions.
    mgr.connect("test", false).await.unwrap();
    assert_eq!(p.asked(), (1, 1, 0));
    mgr.disconnect("test").await;

    // A saved password that fails is kept for the user to deal with.
    home.secrets.put("test", json!({ "password": "stale" }));
    assert!(sign_in_reason(mgr.connect("test", false).await).contains("password for tester@test"));
    assert_eq!(home.secrets.item("test"), Some(json!({ "password": "stale" })));
    assert_eq!(p.asked(), (1, 1, 0));

    // An interactive caller that finds a quiet attempt asks for itself.
    home.secrets.items.lock().unwrap().clear();
    let (quiet, loud) = tokio::join!(mgr.connect("test", false), mgr.backend("test"));
    loud.unwrap();
    assert!(quiet.is_ok() || matches!(quiet, Err(VfsError::SignInNeeded(_))), "{quiet:?}");
    assert_eq!(mgr.status("test"), ConnStatus::Connected);
}

#[tokio::test]
async fn quiet_connect_names_the_jump_host_it_would_ask_about() {
    let jump = TestServer::start(key(1)).await;
    let target = TestServer::start(key(2)).await;
    let home = Home::new();
    home.write("config", &[block("jump", jump.port, ""), block("test", target.port, "  ProxyJump jump\n")].concat());
    let p = trusting_with_password();
    let mgr = home.manager(p.clone());
    assert!(sign_in_reason(mgr.connect("test", false).await).contains("test via jump"));
    assert_eq!(p.asked(), (0, 0, 0));
    assert!(jump.all_closed().await);
}

async fn status_of(rx: &mut tokio::sync::broadcast::Receiver<vpsfiles_ssh::ConnEvent>, within: Duration) -> Option<ConnStatus> {
    tokio::time::timeout(within, rx.recv()).await.ok().map(|e| e.unwrap().status)
}

#[tokio::test]
async fn dropped_connections_come_back_by_themselves() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    home.config(server.port, "");
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), remember: true, ..TestPrompter::trusting() });
    let mgr = home.manager(p.clone());
    mgr.backend("test").await.unwrap();
    let mut rx = mgr.subscribe();

    // Network drop, server down for a while: retried with backoff.
    server.state.refuse.store(true, std::sync::atomic::Ordering::SeqCst);
    server.drop_sessions();
    let s = Duration::from_secs(5);
    assert_eq!(status_of(&mut rx, s).await, Some(ConnStatus::Disconnected));
    assert_eq!(status_of(&mut rx, s).await, Some(ConnStatus::Connecting), "first retry after ~1 s");
    assert!(matches!(status_of(&mut rx, s).await, Some(ConnStatus::Failed { error: VfsError::NotConnected(_) })));
    server.state.refuse.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(status_of(&mut rx, s).await, Some(ConnStatus::Connecting), "second retry after ~2 s");
    assert_eq!(status_of(&mut rx, s).await, Some(ConnStatus::Connected));
    assert_eq!(p.asked(), (1, 1, 0), "the saved password, no prompt");
    mgr.backend("test").await.unwrap().home().await.unwrap();

    // disconnect() means "stay disconnected".
    server.drop_sessions();
    assert_eq!(status_of(&mut rx, s).await, Some(ConnStatus::Disconnected));
    mgr.disconnect("test").await;
    assert_eq!(status_of(&mut rx, Duration::from_millis(1500)).await, None, "no reconnect");
}

#[tokio::test]
async fn background_reconnect_stops_when_it_needs_the_user() {
    let (server, _home, p, mgr) = connected().await; // password typed, not saved
    mgr.backend("test").await.unwrap();
    let mut rx = mgr.subscribe();
    server.drop_sessions();
    let s = Duration::from_secs(5);
    assert_eq!(status_of(&mut rx, s).await, Some(ConnStatus::Disconnected));
    assert_eq!(status_of(&mut rx, s).await, Some(ConnStatus::Connecting));
    assert!(matches!(status_of(&mut rx, s).await, Some(ConnStatus::Failed { error: VfsError::SignInNeeded(_) })));
    let tried = server.count(|c| &c.connections);
    assert_eq!(status_of(&mut rx, Duration::from_millis(2500)).await, None, "no more tries");
    assert_eq!(server.count(|c| &c.connections), tried);
    assert_eq!(p.asked(), (1, 1, 0));
}

// ---- ProxyJump ------------------------------------------------------------

/// A `Host` block pointing at a test server.
fn block(name: &str, port: u16, extra: &str) -> String {
    format!("Host {name}\n  HostName 127.0.0.1\n  Port {port}\n  User {USER}\n{extra}")
}

fn trusting_with_password() -> Arc<TestPrompter> {
    Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..TestPrompter::trusting() })
}

fn forwards(s: &TestServer) -> usize {
    s.count(|c| &c.forwards)
}

#[tokio::test]
async fn proxy_jump_through_one_host() {
    let jump = TestServer::start(key(1)).await;
    let target = TestServer::start(key(2)).await;
    let home = Home::new();
    home.write("config", &[block("jump", jump.port, ""), block("test", target.port, "  ProxyJump jump\n")].concat());
    let p = trusting_with_password();
    let mgr = home.manager(p.clone());
    let mut rx = mgr.subscribe();

    let fs = mgr.backend("test").await.unwrap();
    let (_g, dir) = tmp();
    fs.list(&dir).await.unwrap();
    assert_eq!(mgr.exec("test", "echo hi").await.unwrap().stdout, b"hi\n");
    // Each hop is checked and authenticated on its own.
    assert_eq!(p.asked(), (2, 2, 0));
    let kh = home.known_hosts();
    for port in [jump.port, target.port] {
        assert!(kh.contains(&format!("[127.0.0.1]:{port} ssh-ed25519")), "{kh}");
    }
    // The target is reached only through the jump (TOFU check + real one).
    assert_eq!((forwards(&jump), target.count(|c| &c.connections)), (2, 2));
    assert_eq!(next_status(&mut rx).await, ConnStatus::Connecting);
    assert_eq!(next_status(&mut rx).await, ConnStatus::Connected);

    // The jump host going away takes the target down with it.
    jump.drop_sessions();
    assert_eq!(next_status(&mut rx).await, ConnStatus::Disconnected);
    assert!(target.all_closed().await);
    mgr.backend("test").await.unwrap();
    assert_eq!(p.asked(), (2, 4, 0), "host keys are known now");
    assert_eq!(next_status(&mut rx).await, ConnStatus::Connecting);
    assert_eq!(next_status(&mut rx).await, ConnStatus::Connected);

    // disconnect() closes the jump connection too.
    mgr.disconnect("test").await;
    assert!(jump.all_closed().await && target.all_closed().await);
}

#[tokio::test]
async fn proxy_jump_chains_inline_specs_and_nested_jumps() {
    let a = TestServer::start(key(1)).await;
    let b = TestServer::start(key(2)).await;
    let c = TestServer::start(key(3)).await;
    let home = Home::new();

    // `user@host:port`, then an alias.
    let chain = format!("  ProxyJump {USER}@127.0.0.1:{},b\n", a.port);
    home.write("config", &[block("b", b.port, ""), block("test", c.port, &chain)].concat());
    let p = trusting_with_password();
    home.manager(p.clone()).backend("test").await.unwrap().home().await.unwrap();
    assert_eq!(p.asked(), (3, 3, 0));
    assert_eq!((a.count(|c| &c.connections), forwards(&a), forwards(&b), forwards(&c)), (2, 2, 2, 0));
    assert_eq!(c.count(|c| &c.connections), 2);

    // The first jump's own ProxyJump is followed: test → b, b → a.
    let nested = [block("a", a.port, ""), block("b", b.port, "  ProxyJump a\n"), block("test", c.port, "  ProxyJump b\n")];
    home.write("config", &nested.concat());
    let p = trusting_with_password();
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 3, 0));
    assert_eq!((forwards(&a), forwards(&b)), (3, 3));

    // ...but not a later one's: `ProxyJump a,b` is a → b even though b
    // says "through a".
    home.write("config", &[nested[0].clone(), nested[1].clone(), block("test", c.port, "  ProxyJump a,b\n")].concat());
    let p = trusting_with_password();
    home.manager(p.clone()).backend("test").await.unwrap();
    assert_eq!(p.asked(), (0, 3, 0));
    assert_eq!((a.count(|c| &c.connections), forwards(&a), forwards(&b)), (4, 4, 4));
}

#[tokio::test]
async fn proxy_jump_failures_name_the_hop() {
    let jump = TestServer::start(key(1)).await;
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let home = Home::new();
    let config = [
        block("jump", jump.port, ""),
        block("test", dead, "  ProxyJump jump\n"),
        block("down", dead, ""),
        block("via-down", jump.port, "  ProxyJump down\n"),
        "Host loop1\n  ProxyJump loop2\nHost loop2\n  ProxyJump loop1\n".into(),
        "Host bad\n  ProxyJump tester@\nHost cmd\n  ProxyCommand nc %h %p\n".into(),
    ];
    home.write("config", &config.concat());
    let mgr = home.manager(trusting_with_password());

    // Nothing listens behind the jump; the jump connection is closed again.
    let err = mgr.backend("test").await.err().unwrap();
    assert_eq!(err.code(), "not_connected");
    assert!(err.to_string().contains("jump can't reach 127.0.0.1:"), "{err}");
    assert!(jump.all_closed().await);

    *jump.state.allow_forwarding.lock().unwrap() = false;
    let err = mgr.backend("test").await.err().unwrap();
    assert_eq!(err.code(), "unsupported");
    assert!(err.to_string().contains("jump doesn't allow forwarding"), "{err}");

    let err = mgr.backend("via-down").await.err().unwrap();
    assert_eq!(err.code(), "not_connected");
    assert!(err.to_string().starts_with("not connected: down:"), "{err}");

    // Caught before any network.
    assert_eq!(code(mgr.backend("loop1").await), "unsupported");
    assert_eq!(code(mgr.backend("bad").await), "invalid_name");
    assert_eq!(code(mgr.backend("cmd").await), "unsupported");
    assert!(jump.all_closed().await);
}

// ---- app-managed hosts ----------------------------------------------------

fn draft(alias: &str, port: u16) -> HostDraft {
    HostDraft {
        alias: alias.into(),
        hostname: "127.0.0.1".into(),
        user: Some(USER.into()),
        port,
        identity_file: None,
        password: None,
    }
}

fn listed(mgr: &SshManager) -> Vec<(String, HostOrigin)> {
    mgr.hosts().unwrap().into_iter().map(|h| (h.alias, h.origin)).collect()
}

fn owned(v: &[(&str, HostOrigin)]) -> Vec<(String, HostOrigin)> {
    v.iter().map(|(a, o)| (a.to_string(), *o)).collect()
}

#[tokio::test]
async fn app_hosts_are_listed_validated_and_persisted() {
    use HostOrigin::{App, SshConfig};
    let home = Home::new();
    home.write("config", "Host cfg\n  HostName 192.0.2.1\n");
    let mgr = home.manager(Arc::new(TestPrompter::default()));

    let saved = mgr.save_host(draft("web", 2222), None).await.unwrap();
    assert_eq!((saved.origin, saved.port, saved.user.as_deref()), (App, 2222, Some(USER)));
    assert_eq!(listed(&mgr), owned(&[("cfg", SshConfig), ("web", App)]));
    let hosts = serde_json::to_value(mgr.hosts().unwrap()).unwrap();
    assert_eq!((&hosts[0]["origin"], &hosts[1]["origin"]), (&json!("sshConfig"), &json!("app")));
    // Read back from hosts.json by the next start.
    assert_eq!(listed(&home.manager(Arc::new(TestPrompter::default()))), listed(&mgr));

    assert_eq!(code(mgr.save_host(draft("cfg", 22), None).await), "already_exists");
    assert_eq!(code(mgr.save_host(draft("web", 22), None).await), "already_exists");
    assert_eq!(code(mgr.save_host(draft("bad name", 22), None).await), "invalid_name");
    assert_eq!(code(mgr.save_host(draft("x", 22), Some("cfg")).await), "unsupported");
    assert_eq!(code(mgr.save_host(draft("x", 22), Some("nope")).await), "not_found");
    assert_eq!(code(mgr.remove_host("cfg").await), "unsupported");
    assert_eq!(code(mgr.remove_host("nope").await), "not_found");

    // Rename keeps the position; renaming onto another app host conflicts.
    mgr.save_host(draft("db", 5432), None).await.unwrap();
    mgr.save_host(draft("web2", 2222), Some("web")).await.unwrap();
    assert_eq!(listed(&mgr), owned(&[("cfg", SshConfig), ("web2", App), ("db", App)]));
    assert_eq!(code(mgr.save_host(draft("db", 1), Some("web2")).await), "already_exists");
    mgr.save_host(draft("web2", 2223), Some("web2")).await.unwrap();
    assert_eq!(mgr.hosts().unwrap()[1].port, 2223);

    mgr.remove_host("web2").await.unwrap();
    assert_eq!(listed(&mgr), owned(&[("cfg", SshConfig), ("db", App)]));

    // Every name in ~/.ssh/config is taken, even one not listed because its
    // block sets HostName (`Host cfg cfg-ip` is one server, listed as cfg).
    home.write("config", "Host cfg cfg-ip\n  HostName 192.0.2.1\n");
    assert_eq!(listed(&mgr), owned(&[("cfg", SshConfig), ("db", App)]));
    assert_eq!(code(mgr.save_host(draft("cfg-ip", 22), None).await), "already_exists");

    // ~/.ssh/config defining the same alias later wins.
    home.write("config", "Host cfg db\n");
    assert_eq!(listed(&mgr), owned(&[("cfg", SshConfig), ("db", SshConfig)]));

    // Damaged hosts.json: config hosts still listed, saving won't clobber it.
    std::fs::write(home.path("app/hosts.json"), "{").unwrap();
    assert_eq!(listed(&mgr).len(), 2);
    assert_eq!(code(mgr.save_host(draft("z", 22), None).await), "other");
    assert_eq!(std::fs::read_to_string(home.path("app/hosts.json")).unwrap(), "{");
}

#[tokio::test]
async fn app_host_uses_its_saved_password_and_edits_disconnect() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new(); // no ~/.ssh/config at all
    let p = Arc::new(TestPrompter::trusting()); // has no password to give
    let mgr = home.manager(p.clone());
    mgr.save_host(HostDraft { password: Some(PASSWORD.into()), ..draft("web", server.port) }, None).await.unwrap();
    assert_eq!(home.secrets.item("web"), Some(json!({ "password": PASSWORD })));
    assert!(!std::fs::read_to_string(home.path("app/hosts.json")).unwrap().contains(PASSWORD));
    mgr.backend("web").await.unwrap();
    assert_eq!(p.asked(), (1, 0, 0));

    // Rename: disconnected, the saved password moves along.
    let mut rx = mgr.subscribe();
    assert!(mgr.hosts().unwrap()[0].has_saved_secret);
    mgr.save_host(draft("web2", server.port), Some("web")).await.unwrap();
    let ev = rx.try_recv().unwrap();
    assert_eq!((ev.host.as_str(), ev.status), ("web", ConnStatus::Disconnected));
    assert_eq!((home.secrets.item("web"), home.secrets.item("web2").is_some()), (None, true));
    let index = std::fs::read_to_string(home.path("app/secrets-index.json")).unwrap();
    assert!(index.contains("\"web2\"") && !index.contains("\"web\"") && !index.contains(PASSWORD), "{index}");
    assert!(mgr.hosts().unwrap()[0].has_saved_secret);
    mgr.backend("web2").await.unwrap();
    assert_eq!(p.asked(), (1, 0, 0));

    // A different host name must not get the old password.
    mgr.save_host(HostDraft { hostname: "localhost".into(), ..draft("web2", server.port) }, Some("web2"))
        .await
        .unwrap();
    assert_eq!(mgr.status("web2"), ConnStatus::Disconnected);
    assert_eq!(home.secrets.item("web2"), None);

    // forget_password and remove_host clear the keychain item.
    let with_pw = || HostDraft { password: Some("pw".into()), ..draft("web2", server.port) };
    mgr.save_host(with_pw(), Some("web2")).await.unwrap();
    mgr.forget_password("web2").await.unwrap();
    assert_eq!(home.secrets.item("web2"), None);
    assert!(!mgr.hosts().unwrap()[0].has_saved_secret);
    mgr.save_host(with_pw(), Some("web2")).await.unwrap();
    mgr.remove_host("web2").await.unwrap();
    assert_eq!(home.secrets.item("web2"), None);
    assert!(mgr.hosts().unwrap().is_empty());
}

#[tokio::test]
async fn app_host_with_key_file() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    let k = key(9);
    let path = home.write("my_key", &k.to_openssh(LineEnding::LF).unwrap());
    server.authorize(k.public_key());
    let p = Arc::new(TestPrompter::trusting());
    let mgr = home.manager(p.clone());
    let host = mgr.save_host(HostDraft { identity_file: Some(path.clone()), ..draft("k", server.port) }, None).await;
    assert_eq!(host.unwrap().identity_files, [path]);
    mgr.backend("k").await.unwrap();
    assert_eq!(p.asked(), (1, 0, 0));
}

// ---- SFTP backend ---------------------------------------------------------

#[tokio::test]
async fn sftp_matches_local_backend_semantics() {
    let (_server, _home, _p, mgr) = connected().await;
    let fs = mgr.backend("test").await.unwrap();
    assert!(fs.capabilities().exec);
    assert!(!fs.capabilities().trash);
    assert_eq!(fs.source().to_string(), "ssh:test");
    assert!(fs.home().await.unwrap().starts_with('/'));
    let space = fs.disk_space("/").await.unwrap().unwrap();
    assert_eq!((space.free, space.total), (250 * 4096, 1000 * 4096));

    let (_guard, root) = tmp();
    let sub = path::join(&root, "sub").unwrap();
    fs.mkdir(&sub).await.unwrap();
    assert_eq!(code(fs.mkdir(&sub).await), "already_exists");
    assert_eq!(code(fs.mkdir(&format!("{root}/missing/x")).await), "not_found");

    let file = path::join(&sub, "a.txt").unwrap();
    let mut w = fs.open_write(&file, WriteMode::CreateNew).await.unwrap();
    w.write_all(b"hello world").await.unwrap();
    w.shutdown().await.unwrap();
    drop(w);
    assert_eq!(code(fs.open_write(&file, WriteMode::CreateNew).await), "already_exists");
    assert_eq!(code(fs.open_write(&sub, WriteMode::Truncate).await), "is_a_directory");

    let mut r = fs.open_read(&file, 6).await.unwrap();
    let mut s = String::new();
    r.read_to_string(&mut s).await.unwrap();
    assert_eq!(s, "world");

    // Append continues at the end; Truncate starts over.
    let mut w = fs.open_write(&file, WriteMode::Append).await.unwrap();
    w.write_all(b"!!").await.unwrap();
    w.shutdown().await.unwrap();
    assert_eq!(fs.stat(&file).await.unwrap().size, 13);
    let mut w = fs.open_write(&file, WriteMode::Truncate).await.unwrap();
    w.write_all(b"new").await.unwrap();
    w.shutdown().await.unwrap();
    assert_eq!(fs.stat(&file).await.unwrap().size, 3);
    let mut w = fs.open_write(&file, WriteMode::Append).await.unwrap();
    w.write_all(b" text").await.unwrap();
    w.shutdown().await.unwrap();
    let mut s = String::new();
    fs.open_read(&file, 0).await.unwrap().read_to_string(&mut s).await.unwrap();
    assert_eq!(s, "new text");

    let listing = fs.list(&sub).await.unwrap();
    assert_eq!(listing.len(), 1);
    let e = &listing[0];
    assert_eq!((e.name.as_str(), e.kind, e.size, e.path.as_str()), ("a.txt", EntryKind::File, 8, file.as_str()));
    assert!(e.modified.is_some() && e.mode.is_some() && !e.is_hidden && !e.is_symlink);

    // Errors with precise codes.
    assert_eq!(code(fs.list(&file).await), "not_a_directory");
    assert_eq!(code(fs.list(&format!("{root}/nope")).await), "not_found");
    assert_eq!(code(fs.stat(&format!("{root}/nope")).await), "not_found");
    assert_eq!(code(fs.remove_dir(&sub).await), "not_empty");
    assert_eq!(code(fs.remove_dir(&file).await), "not_a_directory");
    assert_eq!(code(fs.remove_file(&sub).await), "is_a_directory");
    assert_eq!(code(fs.open_read(&format!("{root}/nope"), 0).await), "not_found");

    // rename: fails if the target exists (file or dir), like LocalBackend.
    let moved = path::join(&root, "b.txt").unwrap();
    fs.rename(&file, &moved).await.unwrap();
    assert_eq!(fs.stat(&moved).await.unwrap().size, 8);
    fs.mkdir(&file).await.unwrap();
    assert_eq!(code(fs.rename(&moved, &file).await), "already_exists");
    let empty = path::join(&root, "empty").unwrap();
    fs.mkdir(&empty).await.unwrap();
    assert_eq!(code(fs.rename(&sub, &empty).await), "already_exists");
    assert_eq!(code(fs.rename(&format!("{root}/nope"), &format!("{root}/x")).await), "not_found");

    // Hidden files, symlinks (to file, to dir, broken).
    fs.open_write(&format!("{root}/.dot"), WriteMode::Truncate).await.unwrap().shutdown().await.unwrap();
    std::os::unix::fs::symlink(&moved, format!("{root}/link-file")).unwrap();
    std::os::unix::fs::symlink(&sub, format!("{root}/link-dir")).unwrap();
    std::os::unix::fs::symlink(format!("{root}/gone"), format!("{root}/link-broken")).unwrap();
    let listing = fs.list(&root).await.unwrap();
    let get = |n: &str| listing.iter().find(|e| e.name == n).unwrap().clone();
    assert!(get(".dot").is_hidden);
    let (lf, ld, lb) = (get("link-file"), get("link-dir"), get("link-broken"));
    assert_eq!((lf.kind, lf.size, lf.is_symlink), (EntryKind::File, 8, true));
    assert_eq!((ld.kind, ld.is_symlink), (EntryKind::Dir, true));
    assert_eq!((lb.kind, lb.is_symlink), (EntryKind::Other, true));
    assert_eq!(fs.stat(&ld.path).await.unwrap().kind, EntryKind::Dir);
    assert_eq!(fs.stat(&lb.path).await.unwrap().kind, EntryKind::Other);

    // remove_all through `rm -rf`; a symlink to a dir removes only the link.
    fs.remove_all(&ld.path).await.unwrap();
    assert_eq!(fs.stat(&sub).await.unwrap().kind, EntryKind::Dir);
    fs.remove_all(&sub).await.unwrap();
    assert_eq!(code(fs.stat(&sub).await), "not_found");
    assert_eq!(code(fs.remove_all(&sub).await), "not_found");
    assert_eq!(code(fs.remove_all("/").await), "permission_denied");
}

#[tokio::test]
async fn remove_all_without_shell_walks_over_sftp() {
    let (server, _home, _p, mgr) = connected().await;
    *server.state.allow_exec.lock().unwrap() = false;
    let fs = mgr.backend("test").await.unwrap();
    let (_guard, root) = tmp();
    let tree = format!("{root}/t");
    std::fs::create_dir_all(format!("{tree}/a/b")).unwrap();
    std::fs::write(format!("{tree}/a/b/f"), "x").unwrap();
    std::fs::write(format!("{tree}/g"), "y").unwrap();
    std::os::unix::fs::symlink(&root, format!("{tree}/a/up")).unwrap(); // must not be followed
    fs.remove_all(&tree).await.unwrap();
    assert!(server.count(|c| &c.execs) >= 1);
    assert!(!std::path::Path::new(&tree).exists());
    assert!(std::path::Path::new(&root).exists());
}

#[tokio::test]
async fn big_listing_needs_few_round_trips() {
    let (server, _home, _p, mgr) = connected().await;
    let fs = mgr.backend("test").await.unwrap();
    let (_guard, root) = tmp();
    for i in 0..3000 {
        std::fs::write(format!("{root}/f{i:05}"), "").unwrap();
    }
    for i in 0..5 {
        std::os::unix::fs::symlink(format!("{root}/f0000{i}"), format!("{root}/l{i}")).unwrap();
    }
    let stats_before = server.count(|c| &c.stats);
    let readdirs_before = server.count(|c| &c.readdirs);
    let listing = fs.list(&root).await.unwrap();
    assert_eq!(listing.len(), 3005);
    assert_eq!(server.count(|c| &c.stats) - stats_before, 5, "only symlinks are stat'ed");
    // 31 batches of 100, plus the pipelined READDIRs that come back EOF.
    let readdirs = server.count(|c| &c.readdirs) - readdirs_before;
    assert!((32..=31 + 32).contains(&readdirs), "{readdirs}");
}

async fn bytes_at(fs: &dyn vpsfiles_vfs::Backend, path: &str, offset: u64, n: usize) -> Vec<u8> {
    let mut r = fs.open_read(path, offset).await.unwrap();
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await.unwrap();
    buf
}

async fn eventually(what: &str, f: impl Fn() -> bool) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test]
async fn read_handles_are_reused_while_the_file_is_unchanged() {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    home.config(server.port, "");
    let mut opts = home.options();
    opts.read_handle_idle = Duration::from_millis(500);
    let p = Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..TestPrompter::trusting() });
    let mgr = SshManager::with_options(p, opts);
    let fs = mgr.backend("test").await.unwrap();
    let (_g, root) = tmp();
    let file = format!("{root}/v.bin");
    std::fs::write(&file, vec![1u8; 100_000]).unwrap();
    let opens = || server.count(|c| &c.opens);
    let closes = || server.count(|c| &c.closes);

    // Range-request pattern: stat, read somewhere, drop; again elsewhere.
    fs.stat(&file).await.unwrap();
    assert_eq!(bytes_at(&*fs, &file, 0, 10).await, [1; 10]);
    fs.stat(&file).await.unwrap();
    assert_eq!(bytes_at(&*fs, &file, 99_990, 10).await, [1; 10]);
    assert_eq!((opens(), closes()), (1, 0), "one OPEN, handle kept");

    // Replaced (new inode, other size): the stat no longer vouches for the
    // old handle, which gets closed; the new content is read.
    std::fs::write(format!("{root}/v.new"), vec![2u8; 50_000]).unwrap();
    std::fs::rename(format!("{root}/v.new"), &file).unwrap();
    fs.stat(&file).await.unwrap();
    assert_eq!(bytes_at(&*fs, &file, 49_990, 10).await, [2; 10]);
    assert_eq!(opens(), 2);
    eventually("stale handle closed", || closes() == 1).await;

    // Idle handles expire.
    eventually("idle handle closed", || closes() == 2).await;
}

#[tokio::test]
async fn a_header_read_fetches_no_more_than_it_uses() {
    let (server, _home, _p, mgr) = connected().await;
    let fs = mgr.backend("test").await.unwrap();
    let (_g, root) = tmp();
    let file = format!("{root}/photo.jpg");
    std::fs::write(&file, vec![9u8; 4 << 20]).unwrap();
    let before = server.count(|c| &c.read_bytes);
    let mut head = Vec::new();
    fs.open_read(&file, 0).await.unwrap().take(128 << 10).read_to_end(&mut head).await.unwrap();
    assert_eq!(head.len(), 128 << 10);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.count(|c| &c.read_bytes) - before, 128 << 10, "EXIF-sized read, nothing extra");
}

#[tokio::test]
async fn reads_survive_short_answers_and_sizeless_files() {
    let (server, _home, _p, mgr) = connected().await;
    let fs = mgr.backend("test").await.unwrap();
    let (_g, root) = tmp();
    let file = format!("{root}/odd.bin");
    let data: Vec<u8> = (0..1_000_003u32).map(|i| (i % 249) as u8).collect();
    std::fs::write(&file, &data).unwrap();
    // A server that answers less than asked (an old OpenSSH caps at 64 KiB).
    server.state.max_read.store(10_007, std::sync::atomic::Ordering::SeqCst);
    let mut back = Vec::new();
    fs.open_read(&file, 0).await.unwrap().read_to_end(&mut back).await.unwrap();
    assert!(back == data, "short reads reassembled ({} bytes)", back.len());
    let mut tail = Vec::new();
    fs.open_read(&file, 999_000).await.unwrap().read_to_end(&mut tail).await.unwrap();
    assert!(tail == data[999_000..]);
    server.state.max_read.store(0, std::sync::atomic::Ordering::SeqCst);

    // /proc files say size 0 but have content.
    let mut status = String::new();
    fs.open_read("/proc/self/status", 0).await.unwrap().read_to_string(&mut status).await.unwrap();
    assert!(status.contains("Name:"), "{status:?}");
    // Empty file.
    std::fs::write(format!("{root}/empty"), "").unwrap();
    let mut e = Vec::new();
    fs.open_read(&format!("{root}/empty"), 0).await.unwrap().read_to_end(&mut e).await.unwrap();
    assert!(e.is_empty());
}

#[tokio::test]
async fn large_file_round_trip_and_seek() {
    let (_server, _home, _p, mgr) = connected().await;
    let fs = mgr.backend("test").await.unwrap();
    let (_guard, root) = tmp();
    let file = format!("{root}/big.bin");
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let mut w = fs.open_write(&file, WriteMode::CreateNew).await.unwrap();
    w.write_all(&data).await.unwrap();
    w.shutdown().await.unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), data);

    let mut back = Vec::new();
    fs.open_read(&file, 1_234_567).await.unwrap().read_to_end(&mut back).await.unwrap();
    assert_eq!(back, &data[1_234_567..]);
}

// ---- real host (read-only, opt-in) ----------------------------------------

/// `VPSFILES_TEST_HOST=<alias> cargo test -p vpsfiles-ssh -- --ignored`
/// Uses the real ~/.ssh (never the keychain or the app's hosts.json); only
/// connects, lists home and stats it.
#[tokio::test]
#[ignore]
async fn real_host_read_only() {
    let Ok(alias) = std::env::var("VPSFILES_TEST_HOST") else { return };
    struct NoPrompts;
    #[async_trait::async_trait]
    impl vpsfiles_ssh::Prompter for NoPrompts {
        async fn confirm_host_key(&self, _: &str, _: &str, _: &str) -> bool {
            false
        }
        async fn password(&self, _: &str, _: &str) -> Option<vpsfiles_ssh::Secret> {
            None
        }
        async fn passphrase(&self, _: &str, _: &str) -> Option<vpsfiles_ssh::Secret> {
            None
        }
    }
    let scratch = tempfile::tempdir().unwrap();
    let options = SshOptions {
        hosts_file: scratch.path().join("hosts.json"),
        secrets: Arc::new(MemorySecrets::default()),
        ..SshOptions::default()
    };
    let mgr = SshManager::with_options(Arc::new(NoPrompts), options);
    let fs = mgr.backend(&alias).await.unwrap();
    let home = fs.home().await.unwrap();
    let entries = fs.list(&home).await.unwrap();
    println!("{alias}:{home} has {} entries", entries.len());
    assert_eq!(fs.stat(&home).await.unwrap().kind, EntryKind::Dir);
    mgr.disconnect(&alias).await;
}
