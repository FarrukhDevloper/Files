//! Round-trip budgets and throughput through a proxy that adds 100 ms each
//! way (RTT 200 ms), like a far-away VPS. Hermetic: 127.0.0.1 + temp dirs.
//!
//! `cargo test -p vpsfiles-ssh --test latency -- --nocapture` prints the
//! numbers; `VPSFILES_NO_BUDGET=1` reports without asserting.

#![cfg(unix)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vpsfiles_ssh::SshManager;
use vpsfiles_vfs::{Backend, WriteMode};

const ONE_WAY: Duration = Duration::from_millis(100);
const RTT: f64 = 0.2;

struct Lab {
    server: TestServer,
    _home: Home,
    mgr: SshManager,
    dir: tempfile::TempDir,
}

/// `VPSFILES_TRACE=1`: the per-request trace, as in examples/probe.rs.
fn init_trace() {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::prelude::*;
    if std::env::var_os("VPSFILES_TRACE").is_none() {
        return;
    }
    let targets = Targets::new().with_target("vpsfiles_ssh", LevelFilter::TRACE);
    let fmt = tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_timer(tracing_subscriber::fmt::time::uptime());
    let _ = tracing_subscriber::registry().with(fmt.with_filter(targets)).try_init();
}

/// Server behind the proxy, host key already trusted, password auth.
async fn lab() -> Lab {
    init_trace();
    let server = TestServer::start(key(1)).await;
    // VPSFILES_LINK_MIBPS=6: also cap the link, like the real one.
    let rate = std::env::var("VPSFILES_LINK_MIBPS").ok().and_then(|v| v.parse::<f64>().ok());
    let proxy = LatencyProxy::with_rate(server.port, ONE_WAY, rate.map(|r| (r * 1048576.0) as u64)).await;
    let home = Home::new();
    home.config(proxy.port, "");
    let public = key(1).public_key().to_openssh().unwrap();
    home.write("known_hosts", &format!("[127.0.0.1]:{} {public}\n", proxy.port));
    let prompter = Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..Default::default() });
    let mgr = home.manager(prompter);
    let dir = tempfile::tempdir().unwrap();
    Lab { server, _home: home, mgr, dir }
}

fn rtts(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() / RTT
}

fn report(what: &str, rtts: f64, budget: f64) {
    println!("{what:<34} {rtts:>5.1} RTT   (budget {budget})");
    // Slack for CPU (crypto, debug build) on a busy machine.
    if std::env::var_os("VPSFILES_NO_BUDGET").is_none() {
        assert!(rtts < budget + 0.6, "{what}: {rtts:.2} RTT, budget {budget}");
    }
}

#[tokio::test]
async fn round_trip_budgets() {
    let lab = lab().await;
    let root = lab.dir.path();
    for i in 0..47 {
        std::fs::write(root.join(format!("file{i:02}.txt")), "x").unwrap();
    }
    let links = root.join("links");
    std::fs::create_dir(&links).unwrap();
    for i in 0..10 {
        std::fs::write(links.join(format!("f{i}")), "x").unwrap();
    }
    for i in 0..5 {
        std::os::unix::fs::symlink(links.join(format!("f{i}")), links.join(format!("l{i}"))).unwrap();
    }
    std::fs::write(root.join("small.bin"), vec![7u8; 4096]).unwrap();
    std::fs::write(root.join("big.bin"), vec![1u8; 4 << 20]).unwrap();
    let p = |name: &str| format!("{}/{name}", root.display());

    let t = Instant::now();
    let fs = lab.mgr.backend("test").await.unwrap();
    report("connect (known host, password)", rtts(t), 7.0);

    let t = Instant::now();
    fs.home().await.unwrap();
    report("home", rtts(t), 1.0);

    let t = Instant::now();
    assert_eq!(fs.list(&root.display().to_string()).await.unwrap().len(), 47 + 3);
    report("list 50 entries", rtts(t), 2.0);

    let t = Instant::now();
    assert_eq!(fs.list(&p("links")).await.unwrap().len(), 15);
    report("list 15 entries, 5 symlinks", rtts(t), 3.0);

    let many = root.join("many");
    std::fs::create_dir(&many).unwrap();
    for i in 0..3000 {
        std::fs::write(many.join(format!("f{i:05}")), "").unwrap();
    }
    let t = Instant::now();
    assert_eq!(fs.list(&p("many")).await.unwrap().len(), 3000);
    report("list 3000 entries", rtts(t), 6.0);

    let t = Instant::now();
    fs.stat(&p("small.bin")).await.unwrap();
    report("stat file", rtts(t), 1.0);

    let t = Instant::now();
    fs.stat(&p("links/l0")).await.unwrap();
    report("stat symlink", rtts(t), 1.0);

    let t = Instant::now();
    let mut data = Vec::new();
    fs.open_read(&p("small.bin"), 0).await.unwrap().read_to_end(&mut data).await.unwrap();
    assert_eq!(data.len(), 4096);
    report("read 4 KiB file (open+read+close)", rtts(t), 2.0);

    let t = Instant::now();
    let mut r = fs.open_read(&p("big.bin"), 1 << 20).await.unwrap();
    let mut first = [0u8; 1];
    r.read_exact(&mut first).await.unwrap();
    drop(r);
    report("open_read at 1 MiB + first byte", rtts(t), 2.0);

    // A media Range request: stat, then read through the idle handle.
    let t = Instant::now();
    fs.stat(&p("big.bin")).await.unwrap();
    let mut r = fs.open_read(&p("big.bin"), 3 << 20).await.unwrap();
    r.read_exact(&mut first).await.unwrap();
    drop(r);
    report("stat + reused handle + first byte", rtts(t), 2.0);

    let t = Instant::now();
    let mut w = fs.open_write(&p("new.txt"), WriteMode::CreateNew).await.unwrap();
    w.write_all(b"hello").await.unwrap();
    w.shutdown().await.unwrap();
    report("write 5 B file (open+write+close)", rtts(t), 2.0);

    let t = Instant::now();
    fs.mkdir(&p("newdir")).await.unwrap();
    report("mkdir", rtts(t), 1.0);
    let _ = &lab.server;
}

async fn read_all(fs: &dyn Backend, path: &str) -> usize {
    let mut r = fs.open_read(path, 0).await.unwrap();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0;
    loop {
        let n = r.read(&mut buf).await.unwrap();
        if n == 0 {
            return total;
        }
        total += n;
    }
}

#[tokio::test]
async fn throughput() {
    let size_mib = std::env::var("VPSFILES_THROUGHPUT_MIB").ok().and_then(|v| v.parse().ok()).unwrap_or(16usize);
    let size = size_mib << 20;
    let lab = lab().await;
    let src = lab.dir.path().join("src.bin");
    let data: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    let fs = lab.mgr.backend("test").await.unwrap();
    if let Some(cap) = std::env::var("VPSFILES_SERVER_READ_CAP").ok().and_then(|v| v.parse().ok()) {
        lab.server.state.max_read.store(cap, std::sync::atomic::Ordering::SeqCst);
    }
    let mib = size as f64 / 1048576.0;

    let t = Instant::now();
    assert_eq!(read_all(&*fs, &src.display().to_string()).await, size);
    let read = mib / t.elapsed().as_secs_f64();

    let dst = lab.dir.path().join("dst.bin");
    let t = Instant::now();
    let mut w = fs.open_write(&dst.display().to_string(), WriteMode::CreateNew).await.unwrap();
    for chunk in data.chunks(64 * 1024) {
        w.write_all(chunk).await.unwrap();
    }
    w.shutdown().await.unwrap();
    let write = mib / t.elapsed().as_secs_f64();
    assert!(std::fs::read(&dst).unwrap() == data, "upload corrupted");

    println!("read  {mib:.0} MiB through RTT 200 ms: {read:>6.1} MiB/s");
    println!("write {mib:.0} MiB through RTT 200 ms: {write:>6.1} MiB/s");
    // Was 4.2 / 1.6 MiB/s in round 2. Reads: 64 × 32 KiB per RTT is at most
    // 10 MiB/s here. Writes are capped by the server's channel window
    // (2 MiB here and in OpenSSH).
    if std::env::var_os("VPSFILES_NO_BUDGET").is_none() {
        assert!(read > 5.0, "read {read:.1} MiB/s");
        assert!(write > 3.0, "write {write:.1} MiB/s");
    }
}

/// The server answers READs with less than asked (old OpenSSH caps at
/// 64 KiB): throughput must hold over the whole file.
#[tokio::test]
async fn throughput_with_capped_replies() {
    const SIZE: usize = 32 << 20;
    let lab = lab().await;
    let src = lab.dir.path().join("src.bin");
    let data: Vec<u8> = (0..SIZE).map(|i| (i * 13 % 251) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    let fs = lab.mgr.backend("test").await.unwrap();
    for cap in [64 << 10, 32 << 10] {
        lab.server.state.max_read.store(cap, std::sync::atomic::Ordering::SeqCst);
        let t = Instant::now();
        assert_eq!(read_all(&*fs, &src.display().to_string()).await, SIZE);
        let rate = SIZE as f64 / 1048576.0 / t.elapsed().as_secs_f64();
        println!("read 32 MiB, replies capped at {:>2} KiB: {rate:>6.1} MiB/s", cap >> 10);
        if std::env::var_os("VPSFILES_NO_BUDGET").is_none() {
            assert!(rate > 3.0, "cap {cap}: {rate:.1} MiB/s");
        }
    }
}

/// On a link slower than our requests could fill, a big read keeps the
/// link busy without queueing so much that a stat waits behind it.
#[tokio::test]
async fn slow_link_stays_responsive_during_a_big_read() {
    const LINK: f64 = 6.0;
    const SIZE: usize = 24 << 20;
    let server = TestServer::start(key(1)).await;
    let proxy = LatencyProxy::with_rate(server.port, ONE_WAY, Some((LINK * 1048576.0) as u64)).await;
    let home = Home::new();
    home.config(proxy.port, "");
    let public = key(1).public_key().to_openssh().unwrap();
    home.write("known_hosts", &format!("[127.0.0.1]:{} {public}\n", proxy.port));
    let mgr = home.manager(Arc::new(TestPrompter { password: Some(PASSWORD.into()), ..Default::default() }));
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.bin");
    std::fs::write(&src, vec![3u8; SIZE]).unwrap();
    std::fs::write(dir.path().join("small"), "x").unwrap();
    let fs = mgr.backend("test").await.unwrap();

    let reader = {
        let (fs, path) = (fs.clone(), src.display().to_string());
        tokio::spawn(async move {
            let t = Instant::now();
            let n = read_all(&*fs, &path).await;
            (n, t.elapsed().as_secs_f64())
        })
    };
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t = Instant::now();
    fs.stat(&format!("{}/small", dir.path().display())).await.unwrap();
    let stat = rtts(t);
    let (n, secs) = reader.await.unwrap();
    assert_eq!(n, SIZE);
    let rate = SIZE as f64 / 1048576.0 / secs;
    println!("slow link {LINK} MiB/s: read {rate:.1} MiB/s, stat during it {stat:.1} RTT");
    if std::env::var_os("VPSFILES_NO_BUDGET").is_none() {
        assert!(rate > LINK * 0.6, "read {rate:.1} MiB/s on a {LINK} MiB/s link");
        assert!(stat < 3.0, "stat waited {stat:.1} RTT behind the read");
    }
}
