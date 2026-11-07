//! Read-only latency probe against a real host:
//!   cargo run --release -p vpsfiles-ssh --example probe -- <alias> [file-to-read]
//! Connects (agent/keys only — prompts are declined), lists home, runs
//! `true`, and optionally reads up to 16 MiB of a file to measure throughput.
//!
//! * `VPSFILES_TRACE=1`: every SFTP request with offset, size and timing on
//!   stderr; `=2` adds russh's own debug log (window adjusts...).
//! * `VPSFILES_PROBE_MATRIX=1` (needs a file): reads the file once per
//!   transport/SFTP setting (≤ 16 MiB or 12 s each) and prints MiB/s, plus
//!   how long a stat sent 2 s into the read took. `VPSFILES_PROBE_ROWS=1`
//!   runs only the first row (the default).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use vpsfiles_ssh::{Prompter, Secret, SshManager, SshOptions, SshTuning};

struct Decline;

#[async_trait]
impl Prompter for Decline {
    async fn confirm_host_key(&self, _: &str, _: &str, _: &str) -> bool {
        false
    }
    async fn password(&self, _: &str, _: &str) -> Option<Secret> {
        None
    }
    async fn passphrase(&self, _: &str, _: &str) -> Option<Secret> {
        None
    }
}

fn init_trace() {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::prelude::*;
    let level = std::env::var("VPSFILES_TRACE").unwrap_or_default();
    if level.is_empty() || level == "0" {
        return;
    }
    let mut targets = Targets::new().with_target("vpsfiles_ssh", LevelFilter::TRACE);
    if level == "2" {
        targets = targets.with_target("russh", LevelFilter::DEBUG).with_target("russh_sftp", LevelFilter::DEBUG);
    }
    let fmt = tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_timer(tracing_subscriber::fmt::time::uptime());
    tracing_subscriber::registry().with(fmt.with_filter(targets)).init();
}

struct Read {
    bytes: usize,
    secs: f64,
    open_ms: f64,
    /// A `stat` of home sent 2 s into the read: does it queue behind it?
    stat_ms: Option<f64>,
}

/// Read up to 16 MiB (or for `limit`).
async fn read_some(mgr: &SshManager, alias: &str, file: &str, limit: Duration) -> Read {
    let fs = mgr.backend(alias).await.expect("connect");
    let home = fs.home().await.unwrap();
    let side = {
        let fs = fs.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let t = Instant::now();
            fs.stat(&home).await.ok().map(|_| t.elapsed().as_secs_f64() * 1e3)
        })
    };
    let t = Instant::now();
    let mut r = fs.open_read(file, 0).await.unwrap();
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    let mut buf = vec![0u8; 256 * 1024];
    let mut bytes = 0usize;
    while bytes < 16 << 20 && t.elapsed() < limit {
        let n = r.read(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        bytes += n;
    }
    let secs = t.elapsed().as_secs_f64();
    let stat_ms = if side.is_finished() { side.await.ok().flatten() } else { side.abort(); None };
    Read { bytes, secs, open_ms, stat_ms }
}

async fn matrix(alias: &str, file: &str) {
    let d = SshTuning::default();
    let rows = [
        ("default", d),
        ("128 in flight", SshTuning { requests_in_flight: 128, ..d }),
        ("Nagle on", SshTuning { tcp_nodelay: false, ..d }),
        ("packet 32 KiB", SshTuning { max_packet: 32768, ..d }),
        ("window 2 MiB", SshTuning { channel_window: 2 << 20, ..d }),
        ("chunk 64 KiB", SshTuning { max_chunk: 64 << 10, ..d }),
        ("chunk 128 KiB", SshTuning { max_chunk: 128 << 10, ..d }),
    ];
    println!("{:<16} {:>6} {:>7} {:>8} {:>8}", "setting", "MiB", "MiB/s", "open ms", "stat ms");
    let only = std::env::var("VPSFILES_PROBE_ROWS").ok().and_then(|n| n.parse().ok()).unwrap_or(rows.len());
    for (name, tuning) in rows.into_iter().take(only) {
        let opts = SshOptions { tuning, ..SshOptions::default() };
        let mgr = SshManager::with_options(Arc::new(Decline), opts);
        let r = read_some(&mgr, alias, file, Duration::from_secs(12)).await;
        let mib = r.bytes as f64 / 1048576.0;
        let stat = r.stat_ms.map_or("-".to_string(), |ms| format!("{ms:.0}"));
        println!("{name:<16} {mib:>6.1} {:>7.2} {:>8.0} {stat:>8}", mib / r.secs, r.open_ms);
        mgr.disconnect(alias).await;
    }
}

#[tokio::main]
async fn main() {
    init_trace();
    let mut args = std::env::args().skip(1);
    let alias = args.next().expect("usage: probe <alias> [file]");
    let file = args.next();
    if std::env::var_os("VPSFILES_PROBE_MATRIX").is_some() {
        matrix(&alias, file.as_deref().expect("the matrix needs a file to read")).await;
        return;
    }
    let mgr = SshManager::new(Arc::new(Decline));

    let t = Instant::now();
    let fs = mgr.backend(&alias).await.expect("connect");
    println!("connect + sftp     {:>7.0} ms", t.elapsed().as_secs_f64() * 1e3);

    let t = Instant::now();
    let home = fs.home().await.unwrap();
    println!("home               {:>7.0} ms", t.elapsed().as_secs_f64() * 1e3);

    for _ in 0..2 {
        let t = Instant::now();
        let n = fs.list(&home).await.unwrap().len();
        println!("list home ({n:>4})   {:>7.0} ms", t.elapsed().as_secs_f64() * 1e3);
    }

    let t = Instant::now();
    mgr.exec(&alias, "true").await.unwrap();
    println!("exec `true`        {:>7.0} ms", t.elapsed().as_secs_f64() * 1e3);

    if let Some(f) = file {
        let t = Instant::now();
        let mut r = fs.open_read(&f, 0).await.unwrap();
        let mut buf = vec![0u8; 256 * 1024];
        let (mut total, first) = (0usize, t.elapsed());
        while total < 16 << 20 {
            let n = r.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            total += n;
        }
        let secs = t.elapsed().as_secs_f64();
        println!(
            "read {:.1} MiB       {:>7.0} ms  ({:.1} MiB/s, open {:.0} ms)",
            total as f64 / 1048576.0,
            secs * 1e3,
            total as f64 / 1048576.0 / secs,
            first.as_secs_f64() * 1e3
        );
    }
    mgr.disconnect(&alias).await;
}
