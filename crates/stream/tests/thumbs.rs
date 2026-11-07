//! Thumbnail fast paths end to end over HTTP: EXIF thumbnails, the shared
//! freedesktop cache, the server's own ffmpeg, and dropping work when the
//! client goes away. Local temp dirs only; "remote" hosts are local
//! backends posing as SSH.

mod common;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};
use vpsfiles_stream::{MediaOptions, MediaServer, RemoteExec};
use vpsfiles_vfs::local::{from_native, LocalBackend};
use vpsfiles_vfs::{Backend, BackendResolver, BoxRead, BoxWrite, Entry, Location, Result, SourceId, WriteMode};

/// `LocalBackend` that counts what is read and can pose as an SSH host.
struct Counting {
    inner: LocalBackend,
    source: SourceId,
    read: Arc<AtomicU64>,
    opens: AtomicUsize,
    stat_delay: Duration,
}

impl Counting {
    fn new(source: SourceId) -> Arc<Self> {
        Arc::new(Self {
            inner: LocalBackend::new(),
            source,
            read: Arc::default(),
            opens: AtomicUsize::new(0),
            stat_delay: Duration::ZERO,
        })
    }

    fn bytes_read(&self) -> u64 {
        self.read.load(Ordering::SeqCst)
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }
}

struct CountingRead {
    inner: BoxRead,
    n: Arc<AtomicU64>,
}

impl AsyncRead for CountingRead {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = self.inner.as_mut().poll_read(cx, buf);
        self.n.fetch_add((buf.filled().len() - before) as u64, Ordering::SeqCst);
        r
    }
}

/// A remote path is POSIX, so on Windows the fake server's local temp dir
/// arrives as `/C:/…`; hand the local disk `C:/…`.
fn native(p: &str) -> String {
    let b = p.as_bytes();
    if cfg!(windows) && b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        p[1..].to_string()
    } else {
        p.to_string()
    }
}

#[async_trait]
impl Backend for Counting {
    fn source(&self) -> SourceId {
        self.source.clone()
    }
    async fn home(&self) -> Result<String> {
        self.inner.home().await
    }
    async fn list(&self, p: &str) -> Result<Vec<Entry>> {
        self.inner.list(&native(p)).await
    }
    async fn stat(&self, p: &str) -> Result<Entry> {
        tokio::time::sleep(self.stat_delay).await;
        self.inner.stat(&native(p)).await
    }
    async fn mkdir(&self, p: &str) -> Result<()> {
        self.inner.mkdir(&native(p)).await
    }
    async fn remove_file(&self, p: &str) -> Result<()> {
        self.inner.remove_file(&native(p)).await
    }
    async fn remove_dir(&self, p: &str) -> Result<()> {
        self.inner.remove_dir(&native(p)).await
    }
    async fn rename(&self, a: &str, b: &str) -> Result<()> {
        self.inner.rename(&native(a), &native(b)).await
    }
    async fn open_read(&self, p: &str, offset: u64) -> Result<BoxRead> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let inner = self.inner.open_read(&native(p), offset).await?;
        Ok(Box::pin(CountingRead { inner, n: self.read.clone() }))
    }
    async fn open_write(&self, p: &str, mode: WriteMode) -> Result<BoxWrite> {
        self.inner.open_write(&native(p), mode).await
    }
}

struct Any(Arc<Counting>);

#[async_trait]
impl BackendResolver for Any {
    async fn backend(&self, _: &SourceId) -> Result<Arc<dyn Backend>> {
        Ok(self.0.clone())
    }
}

/// Runs the "remote" command locally with sh; `missing` = a host without
/// ffmpeg (exit 127).
struct LocalExec {
    calls: AtomicUsize,
    missing: bool,
}

#[async_trait]
impl RemoteExec for LocalExec {
    async fn exec(&self, _host: &str, command: &str) -> Result<(u32, Vec<u8>)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.missing {
            return Ok((127, Vec::new()));
        }
        let out = tokio::process::Command::new("sh").arg("-c").arg(command).output().await.unwrap();
        Ok((out.status.code().unwrap_or(255) as u32, out.stdout))
    }
}

struct Lab {
    _dir: tempfile::TempDir,
    root: PathBuf,
    #[cfg_attr(windows, allow(dead_code))] // only the freedesktop test reads it
    shared: PathBuf,
    fs: Arc<Counting>,
    server: MediaServer,
}

async fn start(fs: Arc<Counting>, ffmpeg: Option<PathBuf>, remote: Option<Arc<dyn RemoteExec>>) -> Lab {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let shared = dir.path().join("shared-thumbnails");
    let options = MediaOptions {
        thumb_cache: Some(dir.path().join("cache")),
        shared_thumbnails: Some(shared.clone()),
        ffmpeg,
        remote_exec: remote,
    };
    let server = MediaServer::start_with(Arc::new(Any(fs.clone())), options).await.unwrap();
    Lab { _dir: dir, root, shared, fs, server }
}

fn encode_uri_component(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn thumb_url(server: &MediaServer, loc: &Location, size: u32) -> String {
    let segs: Vec<String> = loc.path.split('/').filter(|s| !s.is_empty()).map(encode_uri_component).collect();
    let source = encode_uri_component(&loc.source.to_string());
    format!("{}/thumb/{size}/{source}/{}", server.base_url(), segs.join("/"))
}

fn decoded(body: &[u8]) -> DynamicImage {
    image::load_from_memory(body).unwrap()
}

/// Mean colour, to tell which picture came back.
fn mean(img: &DynamicImage) -> [u8; 3] {
    let rgb = img.to_rgb8();
    let n = u64::from(rgb.width() * rgb.height());
    let mut sum = [0u64; 3];
    for p in rgb.pixels() {
        for (s, v) in sum.iter_mut().zip(p.0) {
            *s += u64::from(v);
        }
    }
    sum.map(|s| (s / n) as u8)
}

fn jpeg(img: &RgbImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img.clone()).write_with_encoder(JpegEncoder::new_with_quality(&mut out, quality)).unwrap();
    out
}

/// Insert an APP1 Exif segment (IFD0 Orientation, IFD1 thumbnail) after SOI.
fn with_exif(jpeg: &[u8], orientation: u16, thumb: &[u8]) -> Vec<u8> {
    let mut t = b"II*\0\x08\0\0\0".to_vec();
    // IFD0: Orientation; next IFD at 26.
    t.extend_from_slice(&1u16.to_le_bytes());
    t.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0]);
    t.extend_from_slice(&orientation.to_le_bytes());
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&26u32.to_le_bytes());
    // IFD1 at 26: JPEGInterchangeFormat(Length); data at 56.
    t.extend_from_slice(&2u16.to_le_bytes());
    for (tag, v) in [(0x0201u16, 56u32), (0x0202, thumb.len() as u32)] {
        t.extend_from_slice(&tag.to_le_bytes());
        t.extend_from_slice(&[4, 0, 1, 0, 0, 0]);
        t.extend_from_slice(&v.to_le_bytes());
    }
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(thumb);
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(t);
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend(app1);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Noise compresses badly: a big file, like a camera's.
fn noisy(w: u32, h: u32) -> RgbImage {
    let mut x = 0x2545_F491u32;
    RgbImage::from_fn(w, h, |_, _| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        Rgb([(x & 0xFF) as u8, ((x >> 8) & 0xFF) as u8, ((x >> 16) & 0xFF) as u8])
    })
}

#[tokio::test]
async fn exif_thumbnail_needs_only_the_first_bytes() {
    let fs = Counting::new(SourceId::ssh("far"));
    let lab = start(fs, None, None).await;
    let blue = jpeg(&RgbImage::from_pixel(240, 160, Rgb([0, 0, 255])), 90);
    let photo = with_exif(&jpeg(&noisy(2400, 1600), 90), 1, &blue);
    assert!(photo.len() > 2 << 20, "{} bytes", photo.len());
    std::fs::write(lab.root.join("photo.jpg"), &photo).unwrap();
    std::fs::write(lab.root.join("rotated.jpg"), with_exif(&jpeg(&noisy(600, 400), 90), 6, &blue)).unwrap();
    let loc = |name: &str| Location::new(SourceId::ssh("far"), format!("{}/{name}", from_native(&lab.root)));

    let r = request(&thumb_url(&lab.server, &loc("photo.jpg"), 128), "GET", &[]).await;
    assert_eq!(r.status, 200);
    let img = decoded(&r.body);
    assert_eq!(img.dimensions(), (128, 85));
    let [_, _, b] = mean(&img);
    assert!(b > 200, "the embedded (blue) thumbnail: {:?}", mean(&img));
    assert!(lab.fs.bytes_read() <= 128 << 10, "read {} bytes", lab.fs.bytes_read());

    // Too small for 512: the photo itself, all of it.
    let r = request(&thumb_url(&lab.server, &loc("photo.jpg"), 512), "GET", &[]).await;
    let img = decoded(&r.body);
    assert_eq!(img.dimensions(), (512, 341));
    assert!(mean(&img)[2] < 200, "decoded from the photo");
    assert!(lab.fs.bytes_read() >= photo.len() as u64);

    // EXIF orientation 6 turns the embedded thumbnail upright too.
    let r = request(&thumb_url(&lab.server, &loc("rotated.jpg"), 128), "GET", &[]).await;
    assert_eq!(decoded(&r.body).dimensions(), (85, 128));
}

#[cfg(not(windows))]
fn png_text(bytes: &[u8], key: &str) -> Option<String> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    reader.next_frame(&mut buf).ok()?;
    reader.finish().ok()?;
    reader.info().uncompressed_latin1_text.iter().find(|t| t.keyword == key).map(|t| t.text.clone())
}

#[cfg(not(windows))]
fn write_shared(shared: &Path, bucket: &str, uri: &str, mtime: i64, img: &RgbImage) -> PathBuf {
    let dir = shared.join(bucket);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{:x}.png", md5::compute(uri)));
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, img.width(), img.height());
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.add_text_chunk("Thumb::URI".into(), uri.into()).unwrap();
        enc.add_text_chunk("Thumb::MTime".into(), mtime.to_string()).unwrap();
        let mut w = enc.write_header().unwrap();
        w.write_image_data(img.as_raw()).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

// The freedesktop cache is a Linux/BSD feature (off by default elsewhere).
#[cfg(not(windows))]
#[tokio::test]
async fn shared_freedesktop_cache_is_read_and_written() {
    let fs = Counting::new(SourceId::Local);
    let lab = start(fs, None, None).await;
    let file = lab.root.join("red.jpg");
    std::fs::write(&file, jpeg(&RgbImage::from_pixel(1000, 800, Rgb([255, 0, 0])), 90)).unwrap();
    let mtime = std::fs::metadata(&file).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap();
    let mtime = mtime.as_secs() as i64;
    let path = from_native(&file);
    let uri = format!("file://{path}");
    assert!(path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b)), "plain path: {path}");
    let loc = Location::local(&path);

    // Another file manager's (green) thumbnail is used; the file isn't read.
    write_shared(&lab.shared, "normal", &uri, mtime, &RgbImage::from_pixel(128, 102, Rgb([0, 255, 0])));
    let r = request(&thumb_url(&lab.server, &loc, 64), "GET", &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(mean(&decoded(&r.body))[1], 255, "green = from the shared cache");
    assert_eq!(lab.fs.opens(), 0);

    // A stale one (other mtime) is ignored, and replaced by ours.
    let stale = write_shared(&lab.shared, "normal", &uri, mtime - 60, &RgbImage::from_pixel(128, 102, Rgb([0, 255, 0])));
    let r = request(&thumb_url(&lab.server, &loc, 100), "GET", &[]).await;
    let img = decoded(&r.body);
    assert_eq!((img.dimensions(), mean(&img)[0] > 200), ((100, 80), true), "red = decoded");
    for _ in 0..100 {
        if png_text(&std::fs::read(&stale).unwrap(), "Thumb::MTime") == Some(mtime.to_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ours = std::fs::read(&stale).unwrap();
    assert_eq!(png_text(&ours, "Thumb::MTime"), Some(mtime.to_string()));
    assert_eq!(png_text(&ours, "Thumb::URI"), Some(uri));
    assert_eq!(decoded(&ours).dimensions(), (128, 102), "normal = 128 px");
}

fn make_video(path: &Path, ffmpeg: &Path) {
    let ok = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=duration=3:size=320x240:rate=10", "-pix_fmt", "yuv420p"])
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

#[tokio::test]
async fn remote_videos_use_the_servers_ffmpeg_then_fall_back() {
    let Some(ffmpeg) = MediaOptions::default().ffmpeg else {
        eprintln!("ffmpeg not installed: skipped");
        return;
    };
    let exec = Arc::new(LocalExec { calls: AtomicUsize::new(0), missing: false });
    let fs = Counting::new(SourceId::ssh("far"));
    let lab = start(fs, Some(ffmpeg.clone()), Some(exec.clone())).await;
    let video = lab.root.join("clip one's.mp4");
    make_video(&video, &ffmpeg);
    let loc = Location::new(SourceId::ssh("far"), from_native(&video));

    let r = request(&thumb_url(&lab.server, &loc, 64), "GET", &[]).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(decoded(&r.body).dimensions(), (64, 48));
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
    assert_eq!(lab.fs.opens(), 0, "nothing streamed: the frame came over exec");

    // A host without ffmpeg: local ffmpeg reading our media URL, and the
    // host isn't asked again.
    let none = Arc::new(LocalExec { calls: AtomicUsize::new(0), missing: true });
    let fs = Counting::new(SourceId::ssh("far"));
    let lab2 = start(fs, Some(ffmpeg), Some(none.clone())).await;
    let video2 = lab2.root.join("clip.mp4");
    std::fs::copy(&video, &video2).unwrap();
    let loc2 = Location::new(SourceId::ssh("far"), from_native(&video2));
    for size in [64, 96] {
        let r = request(&thumb_url(&lab2.server, &loc2, size), "GET", &[]).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    }
    assert_eq!(none.calls.load(Ordering::SeqCst), 1);
    assert!(lab2.fs.opens() > 0, "streamed to the local ffmpeg");
}

#[tokio::test]
async fn work_stops_when_the_client_goes_away() {
    let fs = Arc::new(Counting {
        inner: LocalBackend::new(),
        source: SourceId::ssh("far"),
        read: Arc::default(),
        opens: AtomicUsize::new(0),
        stat_delay: Duration::from_millis(500),
    });
    let lab = start(fs, None, None).await;
    let file = lab.root.join("big.jpg");
    std::fs::write(&file, jpeg(&noisy(800, 600), 90)).unwrap();
    let url = thumb_url(&lab.server, &Location::new(SourceId::ssh("far"), from_native(&file)), 128);

    // Ask, then hang up while the server is still on the stat.
    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_at(rest.find('/').unwrap());
    let mut sock = tokio::net::TcpStream::connect(authority).await.unwrap();
    sock.write_all(format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(sock);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(lab.fs.opens(), 0, "the image wasn't read for nobody");

    // Still works for a client that waits.
    let r = request(&url, "GET", &[]).await;
    assert_eq!((r.status, lab.fs.opens()), (200, 1));
}
