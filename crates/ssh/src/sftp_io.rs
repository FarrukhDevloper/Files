//! Pipelined file I/O on the raw SFTP session.
//!
//! * [`Reader`] keeps READs outstanding (slow start from ~256 KiB, so a
//!   small read doesn't fetch megabytes), asks FSTAT in the same round trip
//!   to know where the file ends, and hands its handle to [`IdleHandles`]
//!   instead of closing it.
//! * [`Writer`] coalesces small writes into chunks, keeps up to
//!   [`MAX_IN_FLIGHT`] WRITEs outstanding and sends CLOSE right behind the
//!   last WRITE, so `shutdown()` costs one round trip.
//!
//! Defaults follow OpenSSH's sftp (64 × 32 KiB in flight): measured over a
//! 130 ms Wi-Fi link to OpenSSH 9.6 that is full speed and still answers a
//! concurrent stat in ~1 RTT; bigger READs were 3–6× slower, and a
//! delay-based window mistook Wi-Fi jitter for queueing and collapsed.
//!
//! Every request is traced at `trace` level (target `vpsfiles_ssh::io`).

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{ready, Context, Poll};
use std::time::{Duration, Instant};

use futures::stream::{FuturesOrdered, StreamExt};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::RawSftpSession;
use russh_sftp::protocol::{Attrs, Data, FileAttributes, Status, StatusCode};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Requests of one kind in flight (symlink stats while listing).
pub(crate) const MAX_IN_FLIGHT: usize = 64;
/// Per reader/writer; the channel window (16 MiB) is shared by all.
const MAX_IN_FLIGHT_BYTES: u64 = 8 << 20;
/// A reader's first requests; nothing more is asked until all of them are
/// consumed, so a header/EXIF read of this size costs exactly this much.
const FIRST_WINDOW_BYTES: u64 = 128 << 10;
/// Without `limits@openssh.com`: what every server handles.
pub(crate) const DEFAULT_CHUNK: u32 = 32 << 10;
const TRACE: &str = "vpsfiles_ssh::io";

/// Requests in flight: (readers) the first window once, then slow start
/// (+3 per answer, about ×4 per round trip) up to the configured most.
struct Pace {
    window: usize,
    max: usize,
    /// Answers of the first window still to come; until then nothing new
    /// is sent beyond it.
    hold: usize,
    sent: usize,
}

impl Pace {
    fn new(window: usize, max: usize) -> Self {
        Self { window: window.min(max).max(1), max, hold: 0, sent: 0 }
    }

    /// Send the first window, then wait for all of it.
    fn holding(mut self) -> Self {
        self.hold = self.window;
        self
    }

    fn may_send(&self, in_flight: usize) -> bool {
        if self.hold > 0 { self.sent < self.window } else { in_flight < self.window }
    }

    fn sent(&mut self) {
        self.sent += 1;
    }

    /// Requests were dropped (restart): stop waiting for their answers.
    fn release(&mut self) {
        self.hold = 0;
    }

    fn set_max(&mut self, max: usize) {
        self.max = max.max(1);
        self.window = self.window.min(self.max);
    }

    fn answered(&mut self) {
        if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.window = (self.window + 3).min(self.max);
        }
    }
}
/// Idle handles kept at most, per backend and per path.
const MAX_IDLE: usize = 32;
const MAX_IDLE_PER_PATH: usize = 4;

/// Request sizes, raised once the server's `limits@openssh.com` arrive.
pub(crate) struct Tuning {
    max_in_flight: usize,
    max_chunk: u32,
    read_chunk: AtomicU32,
    write_chunk: AtomicU32,
}

impl Tuning {
    pub fn new(max_in_flight: usize, max_chunk: u32) -> Self {
        let first = DEFAULT_CHUNK.min(max_chunk.max(1));
        Self {
            max_in_flight: max_in_flight.max(1),
            max_chunk: max_chunk.max(1),
            read_chunk: AtomicU32::new(first),
            write_chunk: AtomicU32::new(first),
        }
    }

    /// `0` = the server didn't say.
    pub fn set_limits(&self, read_len: u64, write_len: u64) {
        let pick = |n: u64| if n == 0 { DEFAULT_CHUNK.min(self.max_chunk) } else { n.min(u64::from(self.max_chunk)) as u32 };
        self.read_chunk.store(pick(read_len), Ordering::Relaxed);
        self.write_chunk.store(pick(write_len), Ordering::Relaxed);
    }

    fn window_for(&self, chunk: u32, bytes: u64) -> usize {
        (bytes / u64::from(chunk.max(1))).clamp(1, self.max_in_flight as u64) as usize
    }

    /// A writer's pace; readers add [`Pace::holding`].
    fn pace(&self, chunk: u32) -> Pace {
        Pace::new(self.window_for(chunk, FIRST_WINDOW_BYTES), self.window_for(chunk, MAX_IN_FLIGHT_BYTES))
    }

    fn read_chunk(&self) -> u32 {
        self.read_chunk.load(Ordering::Relaxed)
    }

    fn write_chunk(&self) -> u32 {
        self.write_chunk.load(Ordering::Relaxed)
    }
}

/// What a file looked like: a cached handle is reused only while a fresh
/// stat of the path still shows the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub size: u64,
    pub mtime: Option<u32>,
}

impl Stamp {
    pub fn of(a: &FileAttributes) -> Self {
        Self { size: a.size.unwrap_or(0), mtime: a.mtime }
    }
}

fn to_io(e: SftpError) -> io::Error {
    match e {
        SftpError::Status(s) if s.status_code == StatusCode::NoSuchFile => io::Error::new(io::ErrorKind::NotFound, s.error_message),
        SftpError::Status(s) if s.status_code == StatusCode::PermissionDenied => {
            io::Error::new(io::ErrorKind::PermissionDenied, s.error_message)
        }
        SftpError::Status(s) => io::Error::other(if s.error_message.is_empty() { s.status_code.to_string() } else { s.error_message }),
        other => io::Error::other(other.to_string()),
    }
}

fn is_eof(e: &SftpError) -> bool {
    matches!(e, SftpError::Status(s) if s.status_code == StatusCode::Eof)
}

/// Close without waiting for the answer (needs a runtime; skipped without).
pub(crate) fn close_later(raw: &Arc<RawSftpSession>, handle: String) {
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let raw = raw.clone();
        rt.spawn(async move {
            let _ = raw.close(handle).await;
        });
    }
}

// ---- idle read handles -----------------------------------------------------

struct Idle {
    handle: String,
    stamp: Stamp,
    since: Instant,
}

/// Read handles kept open for a while after their reader is dropped, so a
/// seek / Range request / re-read skips OPEN (and CLOSE).
pub(crate) struct IdleHandles {
    raw: Arc<RawSftpSession>,
    keep: Duration,
    map: Mutex<HashMap<String, Vec<Idle>>>,
}

impl IdleHandles {
    pub fn new(raw: Arc<RawSftpSession>, keep: Duration) -> Arc<Self> {
        Arc::new(Self { raw, keep, map: Mutex::new(HashMap::new()) })
    }

    /// An idle handle for `path` opened when the file looked like `stamp`.
    /// Handles from an older version of the file are closed.
    pub fn take(&self, path: &str, stamp: Stamp) -> Option<String> {
        let mut map = self.map.lock().unwrap();
        let list = map.get_mut(path)?;
        let keep = self.keep;
        let (fresh, stale): (Vec<Idle>, Vec<Idle>) =
            list.drain(..).partition(|i| i.stamp == stamp && i.since.elapsed() < keep);
        *list = fresh;
        let found = list.pop().map(|i| i.handle);
        if list.is_empty() {
            map.remove(path);
        }
        drop(map);
        for i in stale {
            close_later(&self.raw, i.handle);
        }
        found
    }

    fn put(self: &Arc<Self>, path: String, handle: String, stamp: Stamp) {
        let mut evicted = Vec::new();
        {
            let mut map = self.map.lock().unwrap();
            let list = map.entry(path).or_default();
            list.push(Idle { handle, stamp, since: Instant::now() });
            if list.len() > MAX_IDLE_PER_PATH {
                evicted.push(list.remove(0));
            }
            let total: usize = map.values().map(Vec::len).sum();
            if total > MAX_IDLE {
                // Oldest first.
                if let Some((p, _)) = map
                    .iter()
                    .filter_map(|(p, l)| l.first().map(|i| (p.clone(), i.since)))
                    .min_by_key(|(_, since)| *since)
                {
                    let list = map.get_mut(&p).unwrap();
                    evicted.push(list.remove(0));
                    if list.is_empty() {
                        map.remove(&p);
                    }
                }
            }
        }
        for i in evicted {
            close_later(&self.raw, i.handle);
        }
        let me: Weak<Self> = Arc::downgrade(self);
        let keep = self.keep;
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                tokio::time::sleep(keep).await;
                if let Some(me) = me.upgrade() {
                    me.expire();
                }
            });
        }
    }

    fn expire(&self) {
        let mut old = Vec::new();
        {
            let mut map = self.map.lock().unwrap();
            map.retain(|_, list| {
                let (keep, gone): (Vec<Idle>, Vec<Idle>) = list.drain(..).partition(|i| i.since.elapsed() < self.keep);
                *list = keep;
                old.extend(gone);
                !list.is_empty()
            });
        }
        for i in old {
            close_later(&self.raw, i.handle);
        }
    }
}

// ---- reader ----------------------------------------------------------------

type ReadFut = Pin<Box<dyn Future<Output = (u64, u32, Instant, Result<Data, SftpError>)> + Send>>;
type AttrsFut = Pin<Box<dyn Future<Output = Result<Attrs, SftpError>> + Send>>;

pub(crate) struct Reader {
    raw: Arc<RawSftpSession>,
    tuning: Arc<Tuning>,
    handle: String,
    /// Where the handle goes when we're done (None = close it).
    idle: Option<(Arc<IdleHandles>, String)>,
    pending: FuturesOrdered<ReadFut>,
    /// Offset of the next READ to send.
    next: u64,
    /// Offset of the next byte to hand out.
    pos: u64,
    buf: Vec<u8>,
    at: usize,
    chunk: u32,
    pace: Pace,
    stamp: Option<Stamp>,
    fstat: Option<AttrsFut>,
    eof: bool,
}

impl Reader {
    /// `stamp`: known from a fresh stat (handle reused); otherwise FSTAT is
    /// asked along with the first READs.
    pub fn new(
        raw: Arc<RawSftpSession>,
        tuning: &Arc<Tuning>,
        handle: String,
        offset: u64,
        stamp: Option<Stamp>,
        idle: Option<(Arc<IdleHandles>, String)>,
    ) -> Self {
        let chunk = tuning.read_chunk();
        let fstat: Option<AttrsFut> = match stamp {
            Some(_) => None,
            None => {
                let (raw, h) = (raw.clone(), handle.clone());
                Some(Box::pin(async move { raw.fstat(h).await }))
            }
        };
        tracing::trace!(target: TRACE, "read open at {offset}, chunk {chunk}, reused {}", stamp.is_some());
        Self {
            raw,
            handle,
            idle,
            pending: FuturesOrdered::new(),
            next: offset,
            pos: offset,
            buf: Vec::new(),
            at: 0,
            chunk,
            pace: tuning.pace(chunk).holding(),
            tuning: tuning.clone(),
            stamp,
            fstat,
            eof: false,
        }
    }

    fn known_end(&self) -> Option<u64> {
        // Size 0 can be a /proc file with content: read until EOF.
        self.stamp.map(|s| s.size).filter(|&s| s > 0)
    }

    fn fill(&mut self) {
        let end = self.known_end();
        while self.pace.may_send(self.pending.len()) && end.is_none_or(|e| self.next < e) {
            self.pace.sent();
            let (raw, h, off, len) = (self.raw.clone(), self.handle.clone(), self.next, self.chunk);
            tracing::trace!(target: TRACE, "read → {off} +{len} (in flight {}, window {})", self.pending.len() + 1, self.pace.window);
            let sent = Instant::now();
            self.pending.push_back(Box::pin(async move { (off, len, sent, raw.read(h, off, len).await) }));
            self.next += u64::from(len);
        }
    }

    fn restart_at(&mut self, offset: u64) {
        self.pending = FuturesOrdered::new();
        self.next = offset;
        self.pace.release();
    }
}

impl AsyncRead for Reader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if let Some(f) = this.fstat.as_mut()
            && let Poll::Ready(r) = f.as_mut().poll(cx)
        {
            this.fstat = None;
            if let Ok(a) = r {
                tracing::trace!(target: TRACE, "read fstat: size {:?}", a.attrs.size);
                this.stamp = Some(Stamp::of(&a.attrs));
            }
        }
        loop {
            if this.at < this.buf.len() {
                let n = out.remaining().min(this.buf.len() - this.at);
                out.put_slice(&this.buf[this.at..this.at + n]);
                this.at += n;
                this.pos += n as u64;
                return Poll::Ready(Ok(()));
            }
            if this.eof || out.remaining() == 0 || this.known_end().is_some_and(|e| this.pos >= e) {
                return Poll::Ready(Ok(()));
            }
            this.fill();
            if this.pending.is_empty() {
                // Known end reached while refilling.
                return Poll::Ready(Ok(()));
            }
            let (off, len, sent, result) = match ready!(this.pending.poll_next_unpin(cx)) {
                Some(r) => r,
                None => return Poll::Ready(Ok(())),
            };
            let ms = sent.elapsed().as_secs_f64() * 1e3;
            match &result {
                Ok(d) => tracing::trace!(target: TRACE, "read ← {off} {} bytes after {ms:.0} ms", d.data.len()),
                Err(e) => tracing::trace!(target: TRACE, "read ← {off} {e} after {ms:.0} ms"),
            }
            debug_assert_eq!(off, this.pos);
            match result {
                Ok(d) if d.data.is_empty() => this.eof = true,
                Ok(d) => {
                    let n = d.data.len() as u32;
                    if n < len {
                        // Short read: the server caps reads lower, or the
                        // file ends (or isn't a plain file). Continue right
                        // after it; the requests behind it are off.
                        this.chunk = this.chunk.min(n.max(4096));
                        this.pace.set_max(this.tuning.window_for(this.chunk, MAX_IN_FLIGHT_BYTES));
                        tracing::debug!(target: TRACE, "short read at {off}: {n} of {len}; chunk now {}", this.chunk);
                        this.restart_at(off + u64::from(n));
                    } else {
                        this.pace.answered();
                    }
                    this.buf = d.data;
                    this.at = 0;
                }
                Err(e) if is_eof(&e) => this.eof = true,
                Err(e) => {
                    this.restart_at(this.pos);
                    return Poll::Ready(Err(to_io(e)));
                }
            }
            if this.eof {
                this.pending = FuturesOrdered::new();
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let handle = std::mem::take(&mut self.handle);
        match (self.idle.take(), self.stamp) {
            (Some((idle, path)), Some(stamp)) => idle.put(path, handle, stamp),
            _ => close_later(&self.raw, handle),
        }
    }
}

// ---- writer ----------------------------------------------------------------

type StatusFut = Pin<Box<dyn Future<Output = Result<Status, SftpError>> + Send>>;

pub(crate) struct Writer {
    raw: Arc<RawSftpSession>,
    /// Empty once CLOSE was sent.
    handle: String,
    offset: u64,
    buf: Vec<u8>,
    chunk: usize,
    pace: Pace,
    /// WRITE acks, then the CLOSE answer; sent in this order.
    acks: FuturesOrdered<StatusFut>,
    closing: bool,
}

impl Writer {
    pub fn new(raw: Arc<RawSftpSession>, tuning: &Tuning, handle: String, offset: u64) -> Self {
        let chunk = tuning.write_chunk();
        tracing::trace!(target: TRACE, "write open at {offset}, chunk {chunk}");
        Self {
            raw,
            handle,
            offset,
            buf: Vec::with_capacity(chunk as usize),
            chunk: chunk as usize,
            pace: tuning.pace(chunk),
            acks: FuturesOrdered::new(),
            closing: false,
        }
    }

    fn send_buf(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let data = std::mem::replace(&mut self.buf, Vec::with_capacity(self.chunk));
        let (raw, h, off) = (self.raw.clone(), self.handle.clone(), self.offset);
        self.offset += data.len() as u64;
        let len = data.len();
        tracing::trace!(target: TRACE, "write → {off} +{len} (in flight {})", self.acks.len() + 1);
        let sent = Instant::now();
        self.acks.push_back(Box::pin(async move {
            let r = raw.write(h, off, data).await;
            tracing::trace!(target: TRACE, "write ← {off} after {:.0} ms", sent.elapsed().as_secs_f64() * 1e3);
            r
        }));
    }

    /// Collect finished acks without waiting; the first error wins. Also
    /// gets freshly queued requests onto the wire.
    fn acked(&mut self, r: Result<Status, SftpError>) -> io::Result<()> {
        self.pace.answered();
        r.map(drop).map_err(to_io)
    }

    fn poll_ready_acks(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        while let Poll::Ready(Some(r)) = self.acks.poll_next_unpin(cx) {
            self.acked(r)?;
        }
        Ok(())
    }

    fn poll_all_acks(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while let Some(r) = ready!(self.acks.poll_next_unpin(cx)) {
            self.acked(r)?;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Writer {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if this.closing {
            return Poll::Ready(Err(io::Error::other("write after shutdown")));
        }
        this.poll_ready_acks(cx)?;
        while this.acks.len() >= this.pace.window {
            match ready!(this.acks.poll_next_unpin(cx)) {
                Some(r) => this.acked(r)?,
                None => break,
            }
        }
        let n = data.len().min(this.chunk - this.buf.len());
        this.buf.extend_from_slice(&data[..n]);
        if this.buf.len() == this.chunk {
            this.send_buf();
            this.poll_ready_acks(cx)?;
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if !this.closing {
            this.send_buf();
        }
        this.poll_all_acks(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if !this.closing {
            this.send_buf();
            let (raw, h) = (this.raw.clone(), std::mem::take(&mut this.handle));
            // Queued after the WRITEs, so it's sent after them too.
            this.acks.push_back(Box::pin(async move { raw.close(h).await }));
            this.closing = true;
        }
        this.poll_all_acks(cx)
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if !self.closing {
            // Not shut down: whatever was buffered is lost, like a crash.
            close_later(&self.raw, std::mem::take(&mut self.handle));
        }
    }
}
