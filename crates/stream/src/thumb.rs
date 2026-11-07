//! Thumbnails for `GET /{token}/thumb/{size}/{source}/{path}`.
//!
//! Images are decoded with `image`; videos get one frame from `ffmpeg`
//! (when installed), read through our own media URL so remote files only
//! cost the ranges ffmpeg asks for. Results go to a disk cache keyed by
//! (source, path, size, mtime, length), so a changed file gets a new one.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};
use sha1::{Digest, Sha1};
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;
use vpsfiles_vfs::{Backend, Entry, EntryKind, Location, Result, SourceId, VfsError};

pub(crate) const MIN_SIZE: u32 = 32;
pub(crate) const MAX_SIZE: u32 = 512;
/// Bigger images get no thumbnail (a fast 404).
const REMOTE_MAX: u64 = 25 << 20;
const LOCAL_MAX: u64 = 100 << 20;
/// Decoded-pixel budget per image (a 48 MP photo needs ~150 MB).
const DECODE_ALLOC: u64 = 256 << 20;
/// Thumbnails rendered at once (images and videos together).
const RENDERS: usize = 2;
const VIDEO_TIMEOUT: Duration = Duration::from_secs(15);
const JPEG_QUALITY: u8 = 82;
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

pub(crate) struct Thumbnailer {
    cache: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
    renders: Semaphore,
    trimmed: AtomicBool,
}

impl Thumbnailer {
    pub fn new(cache: Option<PathBuf>, ffmpeg: Option<PathBuf>) -> Self {
        Self { cache, ffmpeg, renders: Semaphore::new(RENDERS), trimmed: AtomicBool::new(false) }
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
        match kind {
            Kind::Image if entry.size > limit => return Ok(None),
            Kind::Video if self.ffmpeg.is_none() => return Ok(None),
            _ => {}
        }
        let key = cache_key(loc, size, &entry);
        if let Some(t) = self.cached(&key).await {
            return Ok(Some(t));
        }

        let _permit = self.renders.acquire().await.map_err(|e| VfsError::Other(e.to_string()))?;
        // Someone else may have rendered it while we waited.
        if let Some(t) = self.cached(&key).await {
            return Ok(Some(t));
        }
        let source = match kind {
            Kind::Image => read_all(fs, &loc.path, entry.size).await?,
            Kind::Video => match video_frame(self.ffmpeg.as_deref().unwrap(), media_url, size).await? {
                Some(frame) => frame,
                None => return Ok(None),
            },
        };
        let thumb = tokio::task::spawn_blocking(move || render(&source, size))
            .await
            .map_err(|e| VfsError::Other(format!("thumbnail task: {e}")))?;
        match &thumb {
            Some(t) => self.store(&key, t).await,
            None => tracing::debug!("no thumbnail for {}{}", loc.source, loc.path),
        }
        Ok(thumb)
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

async fn read_all(fs: &dyn Backend, path: &str, size: u64) -> Result<Vec<u8>> {
    let mut reader = fs.open_read(path, 0).await?;
    let mut buf = Vec::with_capacity(size as usize);
    // The file may have grown since stat; never read past the limit.
    (&mut reader).take(size).read_to_end(&mut buf).await.map_err(|e| VfsError::from_io(e, path))?;
    Ok(buf)
}

/// Decode, fit into `size`×`size` (never enlarged), honor EXIF rotation,
/// encode as JPEG, or PNG when there is real transparency.
fn render(data: &[u8], size: u32) -> Option<Thumb> {
    let mut reader = ImageReader::new(Cursor::new(data)).with_guessed_format().ok()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(DECODE_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let img = DynamicImage::from_decoder(decoder).ok()?;
    let mut img = if img.width() > size || img.height() > size { img.thumbnail(size, size) } else { img };
    img.apply_orientation(orientation);
    encode(img)
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
