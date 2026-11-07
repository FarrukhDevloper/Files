//! Loopback HTTP server that streams any VFS file, with `Range` support, so
//! the webview's `<video>`/`<audio>` can play and seek remote media without
//! downloading it first.
//!
//! URL shape: `http://127.0.0.1:{port}/{token}/{source}/{path segments}`
//! where `source` is `SourceId`'s string form (`local`, `ssh:tanla`) and
//! every segment is percent-encoded; the path's own `/`s are kept, so
//! relative references (subtitles, HLS segments) resolve. Requests without
//! the random per-launch token get 404.
//!
//! Thumbnails: `/{token}/thumb/{size}/{source}/{path segments}`, size
//! clamped to 32..=512; any query (the UI's `?v=mtime`) is ignored.

mod exif;
mod freedesktop;
mod thumb;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::Response;
use tokio::io::{AsyncRead, AsyncReadExt};
use vpsfiles_vfs::{BackendResolver, EntryKind, Location, Result, SourceId, VfsError};

/// Bytes per body chunk read from the backend.
const CHUNK: usize = 256 * 1024;

/// Runs a shell command on an SSH host; the app implements it with
/// `SshManager::exec`. Used for video thumbnails by the server's ffmpeg.
#[async_trait::async_trait]
pub trait RemoteExec: Send + Sync + 'static {
    /// Exit status and stdout.
    async fn exec(&self, host: &str, command: &str) -> Result<(u32, Vec<u8>)>;
}

/// Thumbnails cost (and the cache may delete) nothing but what is here.
#[derive(Clone)]
pub struct MediaOptions {
    /// Disk cache for thumbnails; `None` = render every time.
    pub thumb_cache: Option<PathBuf>,
    /// The freedesktop.org thumbnail dir (`~/.cache/thumbnails`), read and
    /// written for local files so other file managers' thumbnails count.
    pub shared_thumbnails: Option<PathBuf>,
    /// Local `ffmpeg` for video thumbnails; `None` = no local video frames.
    pub ffmpeg: Option<PathBuf>,
    /// Remote video frames by the host's own ffmpeg (falls back to the
    /// local one where the host has none).
    pub remote_exec: Option<Arc<dyn RemoteExec>>,
}

impl Default for MediaOptions {
    /// `<cache dir>/vps-files/thumbs`, `~/.cache/thumbnails` on Linux and
    /// BSDs, `ffmpeg` from `PATH`, no remote exec.
    fn default() -> Self {
        let freedesktop = cfg!(all(unix, not(target_os = "macos")));
        Self {
            thumb_cache: dirs::cache_dir().map(|d| d.join("vps-files").join("thumbs")),
            shared_thumbnails: dirs::cache_dir().filter(|_| freedesktop).map(|d| d.join("thumbnails")),
            ffmpeg: thumb::find_program("ffmpeg"),
            remote_exec: None,
        }
    }
}

pub struct MediaServer {
    port: u16,
    token: String,
    task: tokio::task::AbortHandle,
}

struct Ctx {
    token: String,
    port: u16,
    resolver: Arc<dyn BackendResolver>,
    thumbs: thumb::Thumbnailer,
}

impl Ctx {
    fn media_url(&self, loc: &Location) -> String {
        media_url(self.port, &self.token, loc)
    }
}

fn base_url(port: u16, token: &str) -> String {
    format!("http://127.0.0.1:{port}/{token}")
}

fn media_url(port: u16, token: &str, loc: &Location) -> String {
    let segments: Vec<String> = loc.path.strip_prefix('/').unwrap_or(&loc.path).split('/').map(encode).collect();
    format!("{}/{}/{}", base_url(port, token), encode(&loc.source.to_string()), segments.join("/"))
}

impl MediaServer {
    /// Bind `127.0.0.1:0` and serve in a background task, with the default
    /// [`MediaOptions`].
    pub async fn start(resolver: Arc<dyn BackendResolver>) -> Result<Self> {
        Self::start_with(resolver, MediaOptions::default()).await
    }

    pub async fn start_with(resolver: Arc<dyn BackendResolver>, options: MediaOptions) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| VfsError::Io(format!("media server: {e}")))?;
        let port = listener.local_addr().map_err(|e| VfsError::Io(format!("media server: {e}")))?.port();
        // 122 random bits from the OS RNG.
        let token = uuid::Uuid::new_v4().simple().to_string();
        if options.ffmpeg.is_none() {
            tracing::info!("ffmpeg not found: no video thumbnails");
        }
        let thumbs =
            thumb::Thumbnailer::new(options.thumb_cache, options.shared_thumbnails, options.ffmpeg, options.remote_exec);
        let ctx = Arc::new(Ctx { token: token.clone(), port, resolver, thumbs });
        let app = axum::Router::new().fallback(serve).with_state(ctx);
        let task = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("media server stopped: {e}");
            }
        })
        .abort_handle();
        Ok(Self { port, token, task })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://127.0.0.1:PORT/TOKEN`, no trailing slash; the UI appends
    /// `/{source}/{path}` or `/thumb/{size}/{source}/{path}`.
    pub fn base_url(&self) -> String {
        base_url(self.port, &self.token)
    }

    pub fn url_for(&self, loc: &Location) -> String {
        media_url(self.port, &self.token, loc)
    }
}

impl Drop for MediaServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ---- URL <-> Location -----------------------------------------------------

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for &b in segment.as_bytes() {
        if unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One path segment. `None` for bad escapes, non-UTF-8, or a decoded `/`
/// or NUL (which would change the path's structure).
fn decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).ok()?;
    (!s.contains(['/', '\0'])).then_some(s)
}

fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn same_token(a: &str, b: &str) -> bool {
    // Constant-time, so response timing doesn't leak a prefix match.
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Debug, PartialEq, Eq)]
enum Request {
    Media(Location),
    Thumb { size: u32, loc: Location },
}

fn parse_request(raw_path: &str, token: &str) -> Option<Request> {
    let (tok, rest) = raw_path.strip_prefix('/')?.split_once('/')?;
    if !same_token(tok, token) {
        return None;
    }
    // Unambiguous: a source is `local` or `ssh:…`.
    let Some(rest) = rest.strip_prefix("thumb/") else { return parse_location(rest).map(Request::Media) };
    let (size, rest) = rest.split_once('/')?;
    if size.is_empty() || size.len() > 6 || !size.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let size = size.parse::<u32>().ok()?.clamp(thumb::MIN_SIZE, thumb::MAX_SIZE);
    Some(Request::Thumb { size, loc: parse_location(rest)? })
}

/// `{source}/{segments}` (the part after the token).
fn parse_location(rest: &str) -> Option<Location> {
    let mut parts = rest.split('/');
    let source: SourceId = decode(parts.next()?)?.parse().ok()?;
    let segments: Vec<String> = parts.map(decode).collect::<Option<_>>()?;
    if segments.iter().any(|s| s == "." || s == "..") {
        return None;
    }
    let rest = segments.join("/");
    // Only local Windows paths are drive-rooted (`C:/x`); everything else
    // is POSIX and gets its leading `/` back.
    let path = match segments.first() {
        Some(first) if cfg!(windows) && source == SourceId::Local && is_drive(first) => {
            if rest.len() == 2 { format!("{rest}/") } else { rest }
        }
        _ => format!("/{rest}"),
    };
    Some(Location::new(source, path))
}

// ---- HTTP -----------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum RangeReq {
    Full,
    /// Inclusive byte range, already clipped to the file.
    Part(u64, u64),
    Unsatisfiable,
}

/// A single `bytes=a-b`, `bytes=a-` or `bytes=-n`. Other units are ignored
/// (full response, per RFC 9110); malformed, multi-part or out-of-file
/// ranges are unsatisfiable.
fn parse_range(value: Option<&HeaderValue>, size: u64) -> RangeReq {
    let Some(value) = value else { return RangeReq::Full };
    let Ok(value) = value.to_str() else { return RangeReq::Unsatisfiable };
    let Some((unit, spec)) = value.trim().split_once('=') else { return RangeReq::Unsatisfiable };
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return RangeReq::Full;
    }
    let Some((a, b)) = spec.trim().split_once('-') else { return RangeReq::Unsatisfiable };
    if spec.contains(',') {
        return RangeReq::Unsatisfiable;
    }
    let num = |s: &str| -> Option<u64> {
        let s = s.trim();
        if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) { None } else { s.parse().ok() }
    };
    let (start, end) = match (a.trim().is_empty(), b.trim().is_empty()) {
        // -n: the last n bytes.
        (true, false) => match num(b) {
            Some(n) if n > 0 && size > 0 => (size.saturating_sub(n), size - 1),
            _ => return RangeReq::Unsatisfiable,
        },
        // a-
        (false, true) => match num(a) {
            Some(s) => (s, size.saturating_sub(1)),
            None => return RangeReq::Unsatisfiable,
        },
        // a-b
        (false, false) => match (num(a), num(b)) {
            (Some(s), Some(e)) if s <= e => (s, e.min(size.saturating_sub(1))),
            _ => return RangeReq::Unsatisfiable,
        },
        (true, true) => return RangeReq::Unsatisfiable,
    };
    if start >= size { RangeReq::Unsatisfiable } else { RangeReq::Part(start, end) }
}

fn plain(status: StatusCode, msg: impl Into<String>) -> Response {
    let mut r = Response::new(Body::from(msg.into()));
    *r.status_mut() = status;
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    r
}

fn error_response(e: VfsError) -> Response {
    let status = match e {
        VfsError::NotFound(_) | VfsError::NotADirectory(_) | VfsError::IsADirectory(_) => StatusCode::NOT_FOUND,
        VfsError::PermissionDenied(_) => StatusCode::FORBIDDEN,
        VfsError::NotConnected(_) | VfsError::Auth(_) | VfsError::HostKey(_) | VfsError::Cancelled => {
            StatusCode::BAD_GATEWAY
        }
        VfsError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    plain(status, e.to_string())
}

fn body_from(reader: impl AsyncRead + Send + Unpin + 'static, len: u64) -> Body {
    let stream = futures::stream::try_unfold((reader.take(len), vec![0u8; CHUNK]), |(mut r, mut buf)| async move {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            return Ok::<_, std::io::Error>(None);
        }
        Ok(Some((Bytes::copy_from_slice(&buf[..n]), (r, buf))))
    });
    Body::from_stream(stream)
}

async fn serve(State(ctx): State<Arc<Ctx>>, method: Method, headers: HeaderMap, uri: Uri) -> Response {
    let Some(request) = parse_request(uri.path(), &ctx.token) else {
        return plain(StatusCode::NOT_FOUND, "not found");
    };
    if method != Method::GET && method != Method::HEAD {
        let mut r = plain(StatusCode::METHOD_NOT_ALLOWED, "GET or HEAD only");
        r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return r;
    }
    let head = method == Method::HEAD;
    let (loc, result) = match request {
        Request::Media(loc) => {
            let r = respond(&ctx, &loc, head, headers.get(header::RANGE)).await;
            (loc, r)
        }
        Request::Thumb { size, loc } => {
            let r = thumbnail(&ctx, &loc, size, head).await;
            (loc, r)
        }
    };
    match result {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("media {}{}: {e}", loc.source, loc.path);
            error_response(e)
        }
    }
}

async fn thumbnail(ctx: &Ctx, loc: &Location, size: u32, head: bool) -> Result<Response> {
    let fs = ctx.resolver.backend(&loc.source).await?;
    let Some(thumb) = ctx.thumbs.get(&*fs, loc, size, &ctx.media_url(loc)).await? else {
        // Final for this file version: the UI's URL changes with its mtime.
        let mut r = plain(StatusCode::NOT_FOUND, "no thumbnail");
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=86400"));
        return Ok(r);
    };
    let len = thumb.bytes.len();
    let content_type = thumb.content_type();
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, len)
        .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
        .body(if head { Body::empty() } else { Body::from(thumb.bytes) })
        .map_err(|e| VfsError::Other(format!("thumbnail response: {e}")))
}

async fn respond(ctx: &Ctx, loc: &Location, head: bool, range: Option<&HeaderValue>) -> Result<Response> {
    let fs = ctx.resolver.backend(&loc.source).await?;
    let entry = fs.stat(&loc.path).await?;
    if entry.kind != EntryKind::File {
        return Err(VfsError::NotFound(loc.path.clone()));
    }
    let size = entry.size;
    let mime = mime_guess::from_path(&loc.path).first_or_octet_stream();

    let mut builder = Response::builder()
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, mime.as_ref());
    let (status, start, len) = match parse_range(range, size) {
        RangeReq::Full => (StatusCode::OK, 0, size),
        RangeReq::Part(a, b) => {
            builder = builder.header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{size}"));
            (StatusCode::PARTIAL_CONTENT, a, b - a + 1)
        }
        RangeReq::Unsatisfiable => {
            let mut r = plain(StatusCode::RANGE_NOT_SATISFIABLE, "range not satisfiable");
            r.headers_mut().insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes */{size}")).unwrap());
            r.headers_mut().insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return Ok(r);
        }
    };
    let body = if head || len == 0 { Body::empty() } else { body_from(fs.open_read(&loc.path, start).await?, len) };
    builder
        .status(status)
        .header(header::CONTENT_LENGTH, len)
        .body(body)
        .map_err(|e| VfsError::Other(format!("media response: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    #[test]
    fn ranges() {
        let r = |s: &str, size| parse_range(Some(&hv(s)), size);
        assert_eq!(parse_range(None, 10), RangeReq::Full);
        assert_eq!(r("bytes=2-5", 10), RangeReq::Part(2, 5));
        assert_eq!(r("bytes=2-", 10), RangeReq::Part(2, 9));
        assert_eq!(r("bytes=-3", 10), RangeReq::Part(7, 9));
        assert_eq!(r("bytes=-30", 10), RangeReq::Part(0, 9));
        assert_eq!(r("bytes=5-500", 10), RangeReq::Part(5, 9));
        assert_eq!(r("BYTES = 0-0", 10), RangeReq::Part(0, 0));
        assert_eq!(r("bytes=10-", 10), RangeReq::Unsatisfiable);
        assert_eq!(r("bytes=5-2", 10), RangeReq::Unsatisfiable);
        assert_eq!(r("bytes=-0", 10), RangeReq::Unsatisfiable);
        assert_eq!(r("bytes=0-", 0), RangeReq::Unsatisfiable);
        assert_eq!(r("bytes=abc", 10), RangeReq::Unsatisfiable);
        assert_eq!(r("bytes=+1-2", 10), RangeReq::Unsatisfiable);
        assert_eq!(r("bytes=0-1,3-4", 10), RangeReq::Unsatisfiable);
        assert_eq!(r("items=0-1", 10), RangeReq::Full);
    }

    #[test]
    fn locations_round_trip() {
        let p = |s: &str| match parse_request(s, "t0k") {
            Some(Request::Media(loc)) => Some(loc),
            Some(Request::Thumb { .. }) => panic!("{s} is not a thumbnail"),
            None => None,
        };
        assert_eq!(p("/t0k/local/home/me/a%20b.mp4"), Some(Location::local("/home/me/a b.mp4")));
        assert_eq!(p("/t0k/ssh%3Atanla/srv/x"), Some(Location::new(SourceId::ssh("tanla"), "/srv/x")));
        assert_eq!(p("/t0k/ssh:tanla/"), Some(Location::new(SourceId::ssh("tanla"), "/")));
        assert_eq!(p("/bad/local/x"), None);
        assert_eq!(p("/t0k"), None);
        assert_eq!(p("/t0k/ftp:x/a"), None);
        assert_eq!(p("/t0k/local/a%2Fb"), None);
        assert_eq!(p("/t0k/local/a/../etc"), None);
        assert_eq!(p("/t0k/local/%zz"), None);
        assert_eq!(p("/t0k/local/%ff"), None);
        // Remote paths are POSIX even if a dir is literally named "C:".
        assert_eq!(p("/t0k/ssh:h/C%3A/x"), Some(Location::new(SourceId::ssh("h"), "/C:/x")));
        // encodeURIComponent leaves !'()* alone.
        assert_eq!(p("/t0k/local/a%20(1)!'*.jpg"), Some(Location::local("/a (1)!'*.jpg")));
    }

    #[test]
    fn thumbnail_requests() {
        let t = |s: &str| parse_request(s, "t0k");
        let thumb = |size, path: &str| Some(Request::Thumb { size, loc: Location::local(path) });
        assert_eq!(t("/t0k/thumb/128/local/a/b.png"), thumb(128, "/a/b.png"));
        assert_eq!(t("/t0k/thumb/1/local/x.png"), thumb(32, "/x.png"));
        assert_eq!(t("/t0k/thumb/99999/local/x.png"), thumb(512, "/x.png"));
        assert_eq!(
            t("/t0k/thumb/64/ssh%3Atanla/srv/v.mp4"),
            Some(Request::Thumb { size: 64, loc: Location::new(SourceId::ssh("tanla"), "/srv/v.mp4") })
        );
        for bad in ["/t0k/thumb/x/local/a", "/t0k/thumb/-1/local/a", "/t0k/thumb//local/a", "/t0k/thumb/64", "/bad/thumb/64/local/a"] {
            assert_eq!(t(bad), None, "{bad}");
        }
    }

    #[test]
    fn encoding() {
        assert_eq!(encode("a b#?%/ü"), "a%20b%23%3F%25%2F%C3%BC");
        assert_eq!(decode("a%20b%23%3F%25%C3%BC").as_deref(), Some("a b#?%ü"));
        assert!(same_token("abc", "abc") && !same_token("abc", "abd") && !same_token("abc", "ab"));
    }
}
