//! End-to-end tests against the in-process server in `support/`. Fully
//! hermetic: 127.0.0.1, temp dirs, no real `~/.ssh`.

#![cfg(unix)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use russh::keys::ssh_key::LineEnding;
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vpsfiles_ssh::{ConnStatus, SshManager};
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
async fn proxy_jump_is_unsupported_without_network() {
    let home = Home::new();
    home.write("config", "Host j\n  HostName 192.0.2.1\n  ProxyJump bastion\n");
    let mgr = home.manager(Arc::new(TestPrompter::default()));
    let err = mgr.backend("j").await.err().unwrap();
    assert_eq!(err.code(), "unsupported");
    assert!(err.to_string().contains("ProxyJump"), "{err}");
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
    assert!(server.count(|c| &c.readdirs) - readdirs_before <= 32);
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
/// Uses the real ~/.ssh; only connects, lists home and stats it.
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
        async fn password(&self, _: &str, _: &str) -> Option<String> {
            None
        }
        async fn passphrase(&self, _: &str, _: &str) -> Option<String> {
            None
        }
    }
    let mgr = SshManager::new(Arc::new(NoPrompts));
    let fs = mgr.backend(&alias).await.unwrap();
    let home = fs.home().await.unwrap();
    let entries = fs.list(&home).await.unwrap();
    println!("{alias}:{home} has {} entries", entries.len());
    assert_eq!(fs.stat(&home).await.unwrap().kind, EntryKind::Dir);
    mgr.disconnect(&alias).await;
}
