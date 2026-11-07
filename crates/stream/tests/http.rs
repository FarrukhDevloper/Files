//! Real HTTP over loopback against `LocalBackend` in a temp dir.

mod common;

use std::sync::Arc;

use common::*;
use vpsfiles_stream::{MediaOptions, MediaServer};
use vpsfiles_vfs::local::{from_native, LocalBackend};
use vpsfiles_vfs::{Location, SourceId};

/// File names full of characters that need percent-encoding. Windows forbids
/// `?` and `*` in file names, so it gets the same names without them.
const CLIP: &str = if cfg!(windows) { "clip #1 100%.mp4" } else { "clip #1 100%?.mp4" };
const PHOTO: &str = if cfg!(windows) { "photo (1)!'.jpg" } else { "photo (1)!'*.jpg" };

async fn setup() -> (tempfile::TempDir, String, MediaServer, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let root = from_native(dir.path());
    let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(dir.path().join(CLIP), &data).unwrap();
    std::fs::write(dir.path().join("ünï cødé.mp3"), b"abc").unwrap();
    std::fs::write(dir.path().join("empty.txt"), b"").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let server = MediaServer::start(Arc::new(LocalOnly(Arc::new(LocalBackend::new())))).await.unwrap();
    (dir, root, server, data)
}

fn loc(root: &str, name: &str) -> Location {
    Location::local(format!("{root}/{name}"))
}

#[tokio::test]
async fn full_and_head() {
    let (_d, root, server, data) = setup().await;
    let url = server.url_for(&loc(&root, CLIP));
    assert!(url.starts_with(&format!("http://127.0.0.1:{}/", server.port())));
    let expected = if cfg!(windows) { "/clip%20%231%20100%25.mp4" } else { "/clip%20%231%20100%25%3F.mp4" };
    assert!(url.ends_with(expected), "{url}");

    let r = request(&url, "GET", &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("video/mp4"));
    assert_eq!(r.header("accept-ranges"), Some("bytes"));
    assert_eq!(r.header("content-length"), Some("1000000"));
    assert_eq!(r.body, data);

    let r = request(&url, "HEAD", &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-length"), Some("1000000"));
    assert!(r.body.is_empty());

    let r = request(&server.url_for(&loc(&root, "ünï cødé.mp3")), "GET", &[]).await;
    assert_eq!((r.status, r.header("content-type"), r.body.as_slice()), (200, Some("audio/mpeg"), &b"abc"[..]));

    let r = request(&server.url_for(&loc(&root, "empty.txt")), "GET", &[]).await;
    assert_eq!((r.status, r.header("content-length"), r.body.len()), (200, Some("0"), 0));
}

#[tokio::test]
async fn ranges() {
    let (_d, root, server, data) = setup().await;
    let url = server.url_for(&loc(&root, CLIP));
    let size = data.len();

    for (spec, a, b) in [
        ("bytes=2-5", 2, 5),
        ("bytes=999990-", 999_990, size - 1),
        ("bytes=-3", size - 3, size - 1),
        ("bytes=500000-2000000", 500_000, size - 1),
        ("bytes=0-", 0, size - 1),
    ] {
        let r = request(&url, "GET", &[("Range", spec)]).await;
        assert_eq!(r.status, 206, "{spec}");
        assert_eq!(r.header("content-range"), Some(format!("bytes {a}-{b}/{size}").as_str()), "{spec}");
        assert_eq!(r.header("content-length"), Some((b - a + 1).to_string().as_str()), "{spec}");
        assert_eq!(r.body, &data[a..=b], "{spec}");
    }

    let r = request(&url, "HEAD", &[("Range", "bytes=10-19")]).await;
    assert_eq!((r.status, r.header("content-length"), r.body.len()), (206, Some("10"), 0));

    for spec in ["bytes=1000000-", "bytes=9-3", "bytes=x-y", "bytes=0-1,4-5", "bytes=-0"] {
        let r = request(&url, "GET", &[("Range", spec)]).await;
        assert_eq!(r.status, 416, "{spec}");
        assert_eq!(r.header("content-range"), Some(format!("bytes */{size}").as_str()), "{spec}");
    }

    // Unknown unit: ignored, full body.
    let r = request(&url, "GET", &[("Range", "items=0-1")]).await;
    assert_eq!((r.status, r.body.len()), (200, size));
}

#[tokio::test]
async fn rejects_bad_requests() {
    let (_d, root, server, _data) = setup().await;
    let good = server.url_for(&loc(&root, CLIP));

    // Wrong or missing token.
    let prefix = format!("http://127.0.0.1:{}/", server.port());
    let after = good.strip_prefix(&prefix).unwrap();
    let (token, rest) = after.split_once('/').unwrap();
    let wrong = format!("{prefix}{}/{rest}", "0".repeat(token.len()));
    assert_eq!(request(&wrong, "GET", &[]).await.status, 404);
    assert_eq!(request(&format!("{prefix}{rest}"), "GET", &[]).await.status, 404);
    assert_eq!(request(&prefix, "GET", &[]).await.status, 404);

    // Missing file, directory, unknown source, method.
    assert_eq!(request(&server.url_for(&loc(&root, "nope.mp4")), "GET", &[]).await.status, 404);
    assert_eq!(request(&server.url_for(&loc(&root, "sub")), "GET", &[]).await.status, 404);
    let remote = server.url_for(&Location::new(SourceId::ssh("down"), "/x.mp4"));
    assert!(remote.contains("/ssh%3Adown/x.mp4"), "{remote}");
    assert_eq!(request(&remote, "GET", &[]).await.status, 502);
    let r = request(&good, "POST", &[]).await;
    assert_eq!((r.status, r.header("allow")), (405, Some("GET, HEAD")));

    // An encoded '/' can't smuggle in extra path structure.
    let smuggled = good.replace("/clip%20", "/sub%2F..%2Fclip%20");
    assert_eq!(request(&smuggled, "GET", &[]).await.status, 404);
}

#[tokio::test]
async fn root_url_shape() {
    let (_d, _root, server, _data) = setup().await;
    let url = server.url_for(&Location::new(SourceId::ssh("tanla"), "/"));
    assert!(url.ends_with("/ssh%3Atanla/"), "{url}");
}

// ---- thumbnails -----------------------------------------------------------

/// JavaScript's encodeURIComponent, as src/lib/ipc/media.ts uses it.
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

/// The URL `thumbUrl()` in media.ts builds.
fn thumb_url(server: &MediaServer, loc: &Location, size: u32) -> String {
    let segs: Vec<String> = loc.path.split('/').filter(|s| !s.is_empty()).map(encode_uri_component).collect();
    let source = encode_uri_component(&loc.source.to_string());
    format!("{}/thumb/{size}/{source}/{}?v=1700000000000", server.base_url(), segs.join("/"))
}

fn image_file(path: &std::path::Path, img: image::DynamicImage) {
    img.save(path).unwrap();
}

fn dims(body: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory(body).unwrap();
    (img.width(), img.height())
}

async fn thumb_server(ffmpeg: Option<std::path::PathBuf>) -> (tempfile::TempDir, MediaServer) {
    let cache = tempfile::tempdir().unwrap();
    let options = MediaOptions {
        thumb_cache: Some(cache.path().join("thumbs")),
        shared_thumbnails: Some(cache.path().join("freedesktop")),
        ffmpeg,
        remote_exec: None,
    };
    let server = MediaServer::start_with(Arc::new(LocalOnly(Arc::new(LocalBackend::new()))), options).await.unwrap();
    (cache, server)
}

#[tokio::test]
async fn image_thumbnails() {
    use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
    let dir = tempfile::tempdir().unwrap();
    let root = from_native(dir.path());
    let p = |name: &str| dir.path().join(name);
    image_file(&p(PHOTO), DynamicImage::ImageRgb8(RgbImage::from_pixel(800, 400, Rgb([9, 99, 199]))));
    image_file(&p("logo.png"), DynamicImage::ImageRgba8(RgbaImage::from_pixel(300, 300, Rgba([0, 0, 0, 0]))));
    std::fs::write(p("notes.txt"), "hi").unwrap();
    std::fs::write(p("broken.png"), "not a png").unwrap();
    std::fs::File::create(p("huge.jpg")).unwrap().set_len(101 << 20).unwrap(); // sparse
    let (cache, server) = thumb_server(None).await;
    let thumbs = cache.path().join("thumbs");
    let cached = || std::fs::read_dir(&thumbs).map(|d| d.count()).unwrap_or(0);

    let base = server.base_url();
    assert!(base.starts_with(&format!("http://127.0.0.1:{}/", server.port())) && !base.ends_with('/'), "{base}");
    assert!(server.url_for(&loc(&root, "logo.png")).starts_with(&format!("{base}/local/")));

    let photo = thumb_url(&server, &loc(&root, PHOTO), 128);
    // encodeURIComponent keeps !'()* literal; only the space is encoded.
    assert!(photo.contains(&format!("{}?v=", PHOTO.replace(' ', "%20"))), "{photo}");
    let r = request(&photo, "GET", &[]).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("content-type"), Some("image/jpeg"));
    assert_eq!(r.header("cache-control"), Some("max-age=31536000, immutable"));
    assert_eq!(dims(&r.body), (128, 64));
    assert_eq!(cached(), 1);

    let r = request(&photo, "HEAD", &[]).await;
    assert_eq!((r.status, r.body.len()), (200, 0));
    assert!(r.header("content-length").unwrap().parse::<usize>().unwrap() > 0);
    assert_eq!(cached(), 1, "served from the cache");

    let r = request(&thumb_url(&server, &loc(&root, "logo.png"), 9999), "GET", &[]).await;
    assert_eq!((r.status, r.header("content-type")), (200, Some("image/png")));
    assert_eq!(dims(&r.body), (300, 300), "clamped to 512, never enlarged");

    // The cache really is what gets served: swap a cached file's content.
    for e in std::fs::read_dir(&thumbs).unwrap() {
        std::fs::write(e.unwrap().path(), std::fs::read(p("logo.png")).unwrap()).unwrap();
    }
    assert_eq!(request(&photo, "GET", &[]).await.header("content-type"), Some("image/png"));

    // A changed file (other mtime/size) gets a fresh thumbnail.
    std::thread::sleep(std::time::Duration::from_millis(20));
    image_file(&p(PHOTO), DynamicImage::ImageRgb8(RgbImage::new(100, 400)));
    let r = request(&photo, "GET", &[]).await;
    assert_eq!((r.header("content-type"), dims(&r.body)), (Some("image/jpeg"), (32, 128)));

    // No thumbnail for these: a 404 the webview may cache.
    for name in ["notes.txt", "broken.png", "huge.jpg"] {
        let r = request(&thumb_url(&server, &loc(&root, name), 64), "GET", &[]).await;
        assert_eq!((r.status, r.header("cache-control")), (404, Some("max-age=86400")), "{name}");
    }
    // Errors are not cached.
    let r = request(&thumb_url(&server, &loc(&root, "missing.jpg"), 64), "GET", &[]).await;
    assert_eq!((r.status, r.header("cache-control")), (404, None));
    let r = request(&thumb_url(&server, &Location::new(SourceId::ssh("down"), "/a.jpg"), 64), "GET", &[]).await;
    assert_eq!(r.status, 502);
}

#[tokio::test]
async fn video_thumbnails() {
    let dir = tempfile::tempdir().unwrap();
    let root = from_native(dir.path());
    let video = loc(&root, "clip one.mp4");

    // Without ffmpeg: no thumbnail, quickly.
    std::fs::write(dir.path().join("clip one.mp4"), b"whatever").unwrap();
    let (_c, server) = thumb_server(None).await;
    assert_eq!(request(&thumb_url(&server, &video, 64), "GET", &[]).await.status, 404);

    let Some(ffmpeg) = MediaOptions::default().ffmpeg else {
        eprintln!("ffmpeg not installed: skipping the real video part");
        return;
    };
    let made = std::process::Command::new(&ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=duration=3:size=320x240:rate=10"])
        .args(["-pix_fmt", "yuv420p"])
        .arg(dir.path().join("clip one.mp4"))
        .status()
        .unwrap();
    assert!(made.success());
    let (_c, server) = thumb_server(Some(ffmpeg)).await;
    let r = request(&thumb_url(&server, &video, 64), "GET", &[]).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("content-type"), Some("image/jpeg"));
    assert_eq!(dims(&r.body), (64, 48));
}
