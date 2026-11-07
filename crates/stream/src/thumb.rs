//! Thumbnails for `GET /{token}/thumb/{size}/{source}/{path}`.
//!
//! Cheapest source first:
//! * our disk cache, keyed by (source, path, size, mtime, length);
//! * for local files, the freedesktop cache other file managers fill (and
//!   that we fill too, see `freedesktop`);
//! * for JPEGs the EXIF thumbnail, from the first 128 KiB only, when it is
//!   big enough and has the photo's shape;
//! * otherwise the decoded image; JPEGs at 1/2, 1/4 or 1/8 scale (DCT
//!   scaling) when that still covers the size;
//! * videos: one frame by the server's own ffmpeg over SSH (only a small
//!   JPEG crosses the network), else by a local ffmpeg reading our media URL.
//!
//! Remote I/O and CPU work have separate limits, and a request whose client
//! went away stops before its next expensive step.

use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageBuffer, ImageDecoder, ImageReader, Limits};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Semaphore;
use vpsfiles_vfs::{Backend, Entry, EntryKind, Location, Result, SourceId, VfsError};

use crate::freedesktop::{self, Source};
use crate::{exif, RemoteExec};

pub(crate) const MIN_SIZE: u32 = 32;
pub(crate) const MAX_SIZE: u32 = 512;
/// Bigger images get no thumbnail (a fast 404).
const REMOTE_MAX: u64 = 25 << 20;
const LOCAL_MAX: u64 = 100 << 20;
/// Read first; holds the EXIF block (and its thumbnail) of a camera JPEG.
const PREFIX: u64 = 128 << 10;
/// Decoded-pixel budget per image (a 48 MP photo needs ~150 MB).
const DECODE_ALLOC: u64 = 256 << 20;
/// Remote reads / remote ffmpeg runs at once, and of those whole-file
/// reads: thumbnails must not crowd out browsing on a slow link.
const REMOTE_IO: usize = 4;
const REMOTE_BULK: usize = 2;
const VIDEO_TIMEOUT: Duration = Duration::from_secs(15);
const REMOTE_VIDEO_TIMEOUT: Duration = Duration::from_secs(25);
const JPEG_QUALITY: u8 = 75;
/// The EXIF thumbnail is used only if its shape matches the photo's (some
/// cameras letterbox it).
const ASPECT_TOLERANCE: f64 = 0.03;
/// Trim the disk cache to `CACHE_TRIM_TO` once it grows past `CACHE_MAX`.
const CACHE_MAX: u64 = 512 << 20;
const CACHE_TRIM_TO: u64 = 384 << 20;
const PNG_MAGIC: &[u8] = b"\x89PNG";

pub(crate) struct Thumb {
    pub bytes: Vec<u8>,
}

impl Thumb {
    pub fn content_type(&self) -> &'static str {
        if self.bytes.starts_with(PNG_MAGIC) { "image/png" } else { "image/jpeg" }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Image,
    Video,
}

impl Kind {
    fn of(path: &str) -> Option<Self> {
        let mime = mime_guess::from_path(path).first()?;
        match (mime.type_().as_str(), mime.subtype().as_str()) {
            ("image", "jpeg" | "png" | "webp" | "gif" | "bmp" | "x-ms-bmp") => Some(Kind::Image),
            ("video", _) => Some(Kind::Video),
            _ => None,
        }
    }
}

/// Set when the request future is dropped (the HTTP client went away), so
/// blocking work that hasn't started yet is skipped.
struct Cancel(Arc<AtomicBool>);

impl Cancel {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    fn flag(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
}

impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct Thumbnailer {
    cache: Option<PathBuf>,
    /// `~/.cache/thumbnails` (freedesktop), for local files.
    shared: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
    remote: Option<Arc<dyn RemoteExec>>,
    /// Hosts whose ffmpeg is missing: use the local one.
    no_remote_ffmpeg: Mutex<HashSet<String>>,
    io: Semaphore,
    bulk: Semaphore,
    cpu: Semaphore,
    trimmed: AtomicBool,
}

impl Thumbnailer {
    pub fn new(
        cache: Option<PathBuf>,
        shared: Option<PathBuf>,
        ffmpeg: Option<PathBuf>,
        remote: Option<Arc<dyn RemoteExec>>,
    ) -> Self {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        Self {
            cache,
            shared,
            ffmpeg,
            remote,
            no_remote_ffmpeg: Mutex::new(HashSet::new()),
            io: Semaphore::new(REMOTE_IO),
            bulk: Semaphore::new(REMOTE_BULK),
            cpu: Semaphore::new(cores),
            trimmed: AtomicBool::new(false),
        }
    }

    /// `Ok(None)`: this file gets no thumbnail (type, size, undecodable);
    /// asking again won't change that until the file does.
    pub async fn get(&self, fs: &dyn Backend, loc: &Location, size: u32, media_url: &str) -> Result<Option<Thumb>> {
        let entry = fs.stat(&loc.path).await?;
        if entry.kind != EntryKind::File {
            return Err(VfsError::NotFound(loc.path.clone()));
        }
        let Some(kind) = Kind::of(&loc.path) else { return Ok(None) };
        let limit = if loc.source == SourceId::Local { LOCAL_MAX } else { REMOTE_MAX };
        if kind == Kind::Image && entry.size > limit {
            return Ok(None);
        }
        let key = cache_key(loc, size, &entry);
        if let Some(t) = self.cached(&key).await {
            return Ok(Some(t));
        }
        let cancel = Cancel::new();
        let thumb = match self.shared_thumbnail(loc, &entry, size, &cancel).await? {
            Some(t) => Some(t),
            None => match kind {
                Kind::Image => self.image(fs, loc, &entry, size, &cancel).await?,
                Kind::Video => self.video(loc, size, media_url, &cancel).await?,
            },
        };
        match &thumb {
            Some(t) => self.store(&key, t).await,
            None => tracing::debug!("no thumbnail for {}{}", loc.source, loc.path),
        }
        Ok(thumb)
    }

    /// Decode on the blocking pool, at most one job per core.
    async fn render(&self, data: Vec<u8>, size: u32, orientation: Option<u16>, cancel: &Cancel) -> Result<Option<Thumb>> {
        Ok(self.render_with(data, size, orientation, None, cancel).await?.map(|r| r.thumb))
    }

    async fn render_with(
        &self,
        data: Vec<u8>,
        size: u32,
        orientation: Option<u16>,
        bucket: Option<u32>,
        cancel: &Cancel,
    ) -> Result<Option<Rendered>> {
        let _cpu = self.cpu.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?;
        let flag = cancel.flag();
        tokio::task::spawn_blocking(move || {
            if flag.load(Ordering::Relaxed) {
                return None;
            }
            render(&data, size, orientation, bucket, &flag)
        })
        .await
        .map_err(|e| VfsError::Other(format!("thumbnail task: {e}")))
    }

    /// Where the freedesktop thumbnail of a local file lives, if we may use
    /// it: (root, what it must match).
    fn shared_for<'a>(&'a self, loc: &'a Location, entry: &Entry) -> Option<(&'a Path, Source<'a>)> {
        let root = self.shared.as_deref()?;
        if loc.source != SourceId::Local || Path::new(&loc.path).starts_with(root) {
            return None;
        }
        let mtime = entry.modified?.div_euclid(1000);
        Some((root, Source { path: &loc.path, mtime, size: entry.size }))
    }

    async fn shared_thumbnail(&self, loc: &Location, entry: &Entry, size: u32, cancel: &Cancel) -> Result<Option<Thumb>> {
        let Some((root, src)) = self.shared_for(loc, entry) else { return Ok(None) };
        let _cpu = self.cpu.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?;
        let (root, path, mtime, file_size, flag) = (root.to_path_buf(), src.path.to_string(), src.mtime, src.size, cancel.flag());
        tokio::task::spawn_blocking(move || {
            if flag.load(Ordering::Relaxed) {
                return None;
            }
            let img = freedesktop::lookup(&root, &Source { path: &path, mtime, size: file_size }, size)?;
            encode(fit(img, size))
        })
        .await
        .map_err(|e| VfsError::Other(format!("thumbnail task: {e}")))
    }

    async fn image(
        &self,
        fs: &dyn Backend,
        loc: &Location,
        entry: &Entry,
        size: u32,
        cancel: &Cancel,
    ) -> Result<Option<Thumb>> {
        let remote = loc.source != SourceId::Local;
        let io = match remote {
            true => Some(self.io.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?),
            false => None,
        };
        let mut reader = fs.open_read(&loc.path, 0).await?;
        let mut data = Vec::with_capacity(entry.size.min(PREFIX) as usize);
        read_into(&mut reader, &mut data, PREFIX, &loc.path).await?;
        if exif::is_jpeg(&data) {
            let info = exif::parse(&data);
            if let Some(r) = info.thumbnail.clone().filter(|r| embedded_fits(&data[r.clone()], info.size, size)) {
                drop((io, reader));
                let embedded = data[r].to_vec();
                if let Some(t) = self.render(embedded, size, info.orientation, cancel).await? {
                    return Ok(Some(t));
                }
                return self.whole_image(fs, loc, entry, size, cancel).await;
            }
        }
        if data.len() as u64 == PREFIX {
            let _bulk = match remote {
                true => Some(self.bulk.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?),
                false => None,
            };
            data.reserve(entry.size.saturating_sub(PREFIX) as usize);
            read_into(&mut reader, &mut data, entry.size.saturating_sub(PREFIX), &loc.path).await?;
        }
        drop((io, reader));
        let Some((root, src)) = self.shared_for(loc, entry) else {
            // `None`: render() reads the orientation from the image itself.
            return self.render(data, size, None, cancel).await;
        };
        // Local: also leave a freedesktop thumbnail for the other apps.
        let (bucket, dir) = freedesktop::bucket_for(size);
        let Some(r) = self.render_with(data, size, None, Some(bucket), cancel).await? else { return Ok(None) };
        if let Some(img) = r.bucket {
            let (root, path, mtime, file_size) = (root.to_path_buf(), src.path.to_string(), src.mtime, src.size);
            let mime = mime_guess::from_path(&path).first_or_octet_stream().to_string();
            tokio::task::spawn_blocking(move || {
                freedesktop::store(&root, dir, &Source { path: &path, mtime, size: file_size }, &mime, &img)
            });
        }
        Ok(Some(r.thumb))
    }

    /// The EXIF thumbnail turned out undecodable: read it all after all.
    async fn whole_image(
        &self,
        fs: &dyn Backend,
        loc: &Location,
        entry: &Entry,
        size: u32,
        cancel: &Cancel,
    ) -> Result<Option<Thumb>> {
        let io = match loc.source != SourceId::Local {
            true => Some((
                self.io.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?,
                self.bulk.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?,
            )),
            false => None,
        };
        let mut reader = fs.open_read(&loc.path, 0).await?;
        let mut data = Vec::with_capacity(entry.size as usize);
        read_into(&mut reader, &mut data, entry.size, &loc.path).await?;
        drop((io, reader));
        self.render(data, size, None, cancel).await
    }

    async fn video(&self, loc: &Location, size: u32, media_url: &str, cancel: &Cancel) -> Result<Option<Thumb>> {
        if let (SourceId::Ssh { host }, Some(remote)) = (&loc.source, &self.remote)
            && !self.no_remote_ffmpeg.lock().unwrap().contains(host)
        {
            let frame = {
                let _io = self.io.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?;
                remote_frame(&**remote, host, &loc.path, size).await?
            };
            match frame {
                RemoteFrame::Frame(jpeg) => return self.render(jpeg, size, None, cancel).await,
                RemoteFrame::NoFrame => return Ok(None),
                RemoteFrame::NoFfmpeg => {
                    tracing::info!("{host}: no ffmpeg on the server; video thumbnails use the local one");
                    self.no_remote_ffmpeg.lock().unwrap().insert(host.clone());
                }
            }
        }
        let Some(ffmpeg) = &self.ffmpeg else { return Ok(None) };
        let frame = {
            // ffmpeg is a CPU job of its own.
            let _cpu = self.cpu.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?;
            video_frame(ffmpeg, media_url, size).await?
        };
        match frame {
            Some(png) => self.render(png, size, None, cancel).await,
            None => Ok(None),
        }
    }

    async fn cached(&self, key: &str) -> Option<Thumb> {
        let bytes = tokio::fs::read(self.cache.as_ref()?.join(key)).await.ok()?;
        (!bytes.is_empty()).then_some(Thumb { bytes })
    }

    async fn store(&self, key: &str, thumb: &Thumb) {
        let Some(dir) = &self.cache else { return };
        let tmp = dir.join(format!("{key}.{}.tmp", uuid::Uuid::new_v4().simple()));
        let written = async {
            tokio::fs::create_dir_all(dir).await?;
            tokio::fs::write(&tmp, &thumb.bytes).await?;
            tokio::fs::rename(&tmp, dir.join(key)).await
        };
        if let Err(e) = written.await {
            tracing::debug!("thumbnail cache {}: {e}", dir.display());
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        if !self.trimmed.swap(true, Ordering::Relaxed) {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || trim(&dir, CACHE_MAX, CACHE_TRIM_TO));
        }
    }
}

/// The EXIF thumbnail covers `size` and has the photo's shape.
fn embedded_fits(embedded: &[u8], photo: Option<(u32, u32)>, size: u32) -> bool {
    let Some((w, h)) = exif::parse(embedded).size else { return false };
    if w.max(h) < size {
        return false;
    }
    match photo {
        Some((pw, ph)) => {
            let (a, b) = (f64::from(w) / f64::from(h), f64::from(pw) / f64::from(ph));
            (a - b).abs() / b <= ASPECT_TOLERANCE
        }
        None => true,
    }
}

async fn read_into(reader: &mut (impl AsyncRead + Unpin), buf: &mut Vec<u8>, max: u64, path: &str) -> Result<()> {
    reader.take(max).read_to_end(buf).await.map_err(|e| VfsError::from_io(e, path))?;
    Ok(())
}

enum RemoteFrame {
    Frame(Vec<u8>),
    NoFrame,
    NoFfmpeg,
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// POSIX sh: one JPEG frame at ~10 % (at most 5 s in) on stdout, or exit
/// 127 without ffmpeg. Commands get </dev/null: sh reads the script from
/// stdin (see [`sh_command`]).
fn remote_ffmpeg_script(path: &str, size: u32) -> String {
    format!(
        r#"command -v ffmpeg >/dev/null 2>&1 || exit 127
f={path}
d=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "file:$f" </dev/null 2>/dev/null)
t=$(awk -v d="$d" 'BEGIN {{ t = (d + 0 > 0) ? d / 10 : 0; if (t > 5) t = 5; printf "%.2f", t }}' </dev/null)
T=; command -v timeout >/dev/null 2>&1 && T="timeout 20"
exec $T ffmpeg -hide_banner -nostdin -v error -ss "$t" -i "file:$f" -map 0:v:0 -frames:v 1 -an -sn -dn \
  -vf "scale='min({size},iw)':'min({size},ih)':force_original_aspect_ratio=decrease" \
  -f image2pipe -c:v mjpeg -q:v 4 pipe:1 </dev/null"#,
        path = shell_quote(path)
    )
}

/// Run `script` with POSIX sh whatever the login shell is (bash, zsh,
/// fish…): shipped base64-encoded, so only sh ever parses it.
fn sh_command(script: &str) -> String {
    format!("echo {} | base64 -d | sh", base64(script.as_bytes()))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            out.push(if i <= chunk.len() { T[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

async fn remote_frame(exec: &dyn RemoteExec, host: &str, path: &str, size: u32) -> Result<RemoteFrame> {
    let command = sh_command(&remote_ffmpeg_script(path, size));
    let run = exec.exec(host, &command);
    let (status, stdout) = tokio::time::timeout(REMOTE_VIDEO_TIMEOUT, run)
        .await
        .map_err(|_| VfsError::Other(format!("{host}: ffmpeg gave no frame in time")))??;
    Ok(match status {
        0 if exif::is_jpeg(&stdout) => RemoteFrame::Frame(stdout),
        127 => RemoteFrame::NoFfmpeg,
        // `timeout` fired: maybe a slow disk, try again later.
        124 => return Err(VfsError::Other(format!("{host}: ffmpeg took too long"))),
        _ => RemoteFrame::NoFrame,
    })
}

fn cache_key(loc: &Location, size: u32, entry: &Entry) -> String {
    let mut h = Sha1::new();
    let modified = entry.modified.unwrap_or(0);
    h.update(format!("v1\0{}\0{}\0{size}\0{modified}\0{}", loc.source, loc.path, entry.size).as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Delete the oldest files once `dir` holds more than `max` bytes.
fn trim(dir: &Path, max: u64, to: u64) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = read
        .filter_map(|e| {
            let e = e.ok()?;
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= max {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= to {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// The answer, and (when asked) the same picture fit to the freedesktop
/// bucket size.
struct Rendered {
    thumb: Thumb,
    bucket: Option<DynamicImage>,
}

fn fit(img: DynamicImage, size: u32) -> DynamicImage {
    if img.width() > size || img.height() > size { img.thumbnail(size, size) } else { img }
}

/// Decode, fit into `size`×`size` (never enlarged), turn upright
/// (`orientation` if given, else the image's own EXIF), encode as JPEG, or
/// PNG when there is real transparency. With `bucket`, also keep the
/// picture fit to that (bigger) size. Stops early once `cancel` is set.
fn render(data: &[u8], size: u32, orientation: Option<u16>, bucket: Option<u32>, cancel: &AtomicBool) -> Option<Rendered> {
    let target = bucket.map_or(size, |b| b.max(size));
    let (img, upright) = decode(data, target, orientation)?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    // Rotating after fitting: the box is square, and it's far cheaper.
    let mut big = fit(img, target);
    big.apply_orientation(upright);
    if bucket.is_none() {
        return Some(Rendered { thumb: encode(big)?, bucket: None });
    }
    let small = if big.width() > size || big.height() > size { big.thumbnail(size, size) } else { big.clone() };
    Some(Rendered { thumb: encode(small)?, bucket: Some(big) })
}

/// The picture (at least `target` big where possible) and how to turn it
/// upright.
fn decode(data: &[u8], target: u32, orientation: Option<u16>) -> Option<(DynamicImage, Orientation)> {
    let given = orientation.and_then(|o| Orientation::from_exif(o as u8));
    if exif::is_jpeg(data) {
        let info = exif::parse(data);
        if info.plain_dct && let Some(img) = decode_jpeg_scaled(data, target) {
            let own = info.orientation.and_then(|o| Orientation::from_exif(o as u8));
            return Some((img, given.or(own).unwrap_or(Orientation::NoTransforms)));
        }
    }
    let mut reader = ImageReader::new(Cursor::new(data)).with_guessed_format().ok()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(DECODE_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let own = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let img = DynamicImage::from_decoder(decoder).ok()?;
    Some((img, given.unwrap_or(own)))
}

/// JPEG decoded at the smallest DCT scale (1/8, 1/4, 1/2, 1) that still
/// covers `target`: a 24 MP photo for a 128 px thumbnail costs 1/64 of a
/// full decode. Only for `JpegInfo::plain_dct` input.
fn decode_jpeg_scaled(data: &[u8], target: u32) -> Option<DynamicImage> {
    let mut d = jpeg_decoder::Decoder::new(Cursor::new(data));
    d.read_info().ok()?;
    let t = u16::try_from(target).unwrap_or(u16::MAX);
    let (w, h) = d.scale(t, t).ok()?;
    if u64::from(w) * u64::from(h) * 3 > DECODE_ALLOC {
        return None;
    }
    d.set_max_decoding_buffer_size(DECODE_ALLOC as usize);
    let pixels = d.decode().ok()?;
    let (w, h) = (u32::from(w), u32::from(h));
    match d.info()?.pixel_format {
        jpeg_decoder::PixelFormat::L8 => ImageBuffer::from_raw(w, h, pixels).map(DynamicImage::ImageLuma8),
        jpeg_decoder::PixelFormat::RGB24 => ImageBuffer::from_raw(w, h, pixels).map(DynamicImage::ImageRgb8),
        _ => None,
    }
}

fn encode(img: DynamicImage) -> Option<Thumb> {
    let mut bytes = Vec::new();
    let rgba = img.color().has_alpha().then(|| img.to_rgba8()).filter(|p| p.pixels().any(|p| p.0[3] < 255));
    match rgba {
        Some(rgba) => DynamicImage::ImageRgba8(rgba).write_with_encoder(PngEncoder::new(&mut bytes)),
        None => DynamicImage::ImageRgb8(img.to_rgb8())
            .write_with_encoder(JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)),
    }
    .ok()?;
    Some(Thumb { bytes })
}

fn ffmpeg_command(ffmpeg: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window flashing up from the GUI app.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// `Duration: 00:01:02.50,` from ffmpeg's input summary, in seconds.
fn parse_duration(stderr: &str) -> Option<f64> {
    let rest = &stderr[stderr.find("Duration: ")? + "Duration: ".len()..];
    let hms = rest.split([',', '\n']).next()?.trim();
    let mut secs = 0.0;
    for part in hms.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    (hms.matches(':').count() == 2 && secs.is_finite()).then_some(secs)
}

/// One frame at ~10 % (at most 5 s in), as PNG no bigger than `size`.
/// `None`: ffmpeg found no video frame.
async fn video_frame(ffmpeg: &Path, url: &str, size: u32) -> Result<Option<Vec<u8>>> {
    let run = async {
        // Exits non-zero ("no output file") after printing the duration.
        let probe = ffmpeg_command(ffmpeg).args(["-hide_banner", "-nostdin", "-i", url]).output().await?;
        let at = parse_duration(&String::from_utf8_lossy(&probe.stderr)).map_or(0.0, |d| (d / 10.0).min(5.0));
        let scale = format!("scale='min({size},iw)':'min({size},ih)':force_original_aspect_ratio=decrease");
        let at = format!("{at:.2}");
        let out = ffmpeg_command(ffmpeg)
            .args(["-hide_banner", "-nostdin", "-v", "error", "-ss", &at, "-i", url])
            .args(["-map", "0:v:0", "-frames:v", "1", "-an", "-sn", "-dn", "-vf", &scale])
            .args(["-f", "image2pipe", "-c:v", "png", "pipe:1"])
            .output()
            .await?;
        if !out.status.success() {
            tracing::debug!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok::<_, std::io::Error>((out.status.success() && !out.stdout.is_empty()).then_some(out.stdout))
    };
    match tokio::time::timeout(VIDEO_TIMEOUT, run).await {
        Ok(Ok(frame)) => Ok(frame),
        Ok(Err(e)) => Err(VfsError::Io(format!("{}: {e}", ffmpeg.display()))),
        // kill_on_drop stops the ffmpeg that was still running.
        Err(_) => Err(VfsError::Other(format!("ffmpeg gave no frame within {}s", VIDEO_TIMEOUT.as_secs()))),
    }
}

/// `name` on `PATH`, plus the usual install dirs a GUI app's PATH may lack
/// (Homebrew on macOS).
pub(crate) fn find_program(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let extra: &[&str] = if cfg!(unix) { &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] } else { &[] };
    std::env::split_paths(&path)
        .chain(extra.iter().map(PathBuf::from))
        .map(|dir| dir.join(&exe))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, Rgb, RgbImage, Rgba, RgbaImage};

    fn decoded(t: &Thumb) -> DynamicImage {
        image::load_from_memory(&t.bytes).unwrap()
    }

    fn render(data: &[u8], size: u32) -> Option<Thumb> {
        super::render(data, size, None, None, &AtomicBool::new(false)).map(|r| r.thumb)
    }

    #[test]
    fn given_orientation_and_cancel() {
        let mut jpg = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::new(200, 100)).write_with_encoder(JpegEncoder::new(&mut jpg)).unwrap();
        // 6 = rotate 90°: landscape becomes portrait.
        let t = super::render(&jpg, 100, Some(6), None, &AtomicBool::new(false)).unwrap().thumb;
        assert_eq!(decoded(&t).dimensions(), (50, 100));
        assert!(super::render(&jpg, 100, None, None, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn jpegs_decode_at_the_smallest_covering_scale() {
        let mut jpg = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::from_pixel(1600, 1200, Rgb([10, 200, 30])))
            .write_with_encoder(JpegEncoder::new(&mut jpg))
            .unwrap();
        assert!(exif::parse(&jpg).plain_dct);
        assert_eq!(decode_jpeg_scaled(&jpg, 128).unwrap().dimensions(), (200, 150), "1/8");
        assert_eq!(decode_jpeg_scaled(&jpg, 256).unwrap().dimensions(), (400, 300), "1/4");
        assert_eq!(decode_jpeg_scaled(&jpg, 1600).unwrap().dimensions(), (1600, 1200), "full");
        let t = render(&jpg, 128).unwrap();
        assert_eq!(decoded(&t).dimensions(), (128, 96));
    }

    #[test]
    fn damaged_images_never_panic() {
        // Release builds abort on panic: every decoder path must just fail.
        let mut jpg = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::from_fn(64, 48, |x, y| Rgb([x as u8, y as u8, 7])))
            .write_with_encoder(JpegEncoder::new(&mut jpg))
            .unwrap();
        for cut in (0..jpg.len()).step_by(37) {
            let _ = render(&jpg[..cut], 32);
        }
        for i in 2..jpg.len().min(700) {
            for v in [0x00, 0x01, 0x02, 0xFF] {
                let mut bad = jpg.clone();
                bad[i] = v;
                let _ = render(&bad, 32);
            }
        }
    }

    #[test]
    fn embedded_thumbnail_must_cover_and_match_shape() {
        let mut small = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::new(160, 120)).write_with_encoder(JpegEncoder::new(&mut small)).unwrap();
        assert!(embedded_fits(&small, Some((4000, 3000)), 128));
        assert!(embedded_fits(&small, None, 160));
        assert!(!embedded_fits(&small, Some((4000, 3000)), 256), "too small");
        assert!(!embedded_fits(&small, Some((6000, 4000)), 128), "letterboxed 4:3 for a 3:2 photo");
        assert!(!embedded_fits(b"junk", None, 32));
    }

    #[test]
    fn remote_script_survives_any_login_shell() {
        assert_eq!((base64(b"f"), base64(b"fo"), base64(b"foo")), ("Zg==".into(), "Zm8=".into(), "Zm9v".into()));
        // Only base64 text reaches the login shell; sh gets the exact path.
        let path = "/v/it's a $(clip) `x`.mp4";
        let script = remote_ffmpeg_script(path, 64)
            .replace("command -v ffmpeg >/dev/null 2>&1 || exit 127", ":")
            .replace("d=$(ffprobe", "printf '%s' \"$f\"; exit 3; d=$(ffprobe");
        let cmd = sh_command(&script);
        assert!(cmd.bytes().all(|b| b.is_ascii_alphanumeric() || b" |+/=-".contains(&b)), "{cmd}");
        let out = std::process::Command::new("sh").arg("-c").arg(&cmd).output().unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout), path);
    }

    #[test]
    fn kinds() {
        assert!(Kind::of("/a/b.JPG") == Some(Kind::Image));
        assert!(Kind::of("/a.webp") == Some(Kind::Image));
        assert!(Kind::of("/a.mkv") == Some(Kind::Video));
        assert!(Kind::of("/a.svg").is_none() && Kind::of("/a.txt").is_none() && Kind::of("/noext").is_none());
    }

    #[test]
    fn renders_fit_and_pick_format() {
        let mut jpg = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::from_pixel(800, 400, Rgb([200, 10, 10])))
            .write_with_encoder(JpegEncoder::new(&mut jpg))
            .unwrap();
        let t = render(&jpg, 128).unwrap();
        assert_eq!(t.content_type(), "image/jpeg");
        assert_eq!(decoded(&t).dimensions(), (128, 64));
        // Small images are not enlarged.
        assert_eq!(decoded(&render(&jpg, 512).unwrap()).dimensions(), (512, 256));
        let mut small = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::new(40, 30)).write_with_encoder(PngEncoder::new(&mut small)).unwrap();
        assert_eq!(decoded(&render(&small, 128).unwrap()).dimensions(), (40, 30));

        // RGBA: PNG only if something is actually transparent.
        let png = |px: Rgba<u8>| {
            let mut v = Vec::new();
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, px))
                .write_with_encoder(PngEncoder::new(&mut v))
                .unwrap();
            v
        };
        assert_eq!(render(&png(Rgba([0, 0, 0, 100])), 32).unwrap().content_type(), "image/png");
        assert_eq!(render(&png(Rgba([0, 0, 0, 255])), 32).unwrap().content_type(), "image/jpeg");

        assert!(render(b"not an image", 64).is_none());
    }

    #[test]
    fn durations() {
        let s = "Input #0, mov\n  Duration: 00:01:02.50, start: 0.000000, bitrate: 5 kb/s\n";
        assert_eq!(parse_duration(s), Some(62.5));
        assert_eq!(parse_duration("  Duration: 01:00:00.00\n"), Some(3600.0));
        assert_eq!(parse_duration("  Duration: N/A, bitrate: N/A"), None);
        assert_eq!(parse_duration("nothing"), None);
    }

    #[test]
    fn trim_drops_oldest() {
        let dir = tempfile::tempdir().unwrap();
        for (i, name) in ["a", "b", "c", "d"].iter().enumerate() {
            let p = dir.path().join(name);
            std::fs::write(&p, [0u8; 100]).unwrap();
            let t = std::time::UNIX_EPOCH + Duration::from_secs(1_000 + i as u64);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        }
        trim(dir.path(), 1_000, 250);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 4, "under the limit: untouched");
        trim(dir.path(), 300, 250);
        let mut left: Vec<_> =
            std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["c", "d"]);
    }
}
