//! The freedesktop.org thumbnail cache (`~/.cache/thumbnails`), shared with
//! Nautilus, Dolphin & co: their thumbnails make ours instant and ours
//! help them. Thumbnail Managing Standard 0.9: `<md5(uri)>.png` in
//! `normal` (128), `large` (256), `x-large` (512); a thumbnail is valid only
//! while its `Thumb::URI` / `Thumb::MTime` (and `Thumb::Size`) match.
//! Local files only. All functions block.

use std::io::{Cursor, Write};
use std::path::Path;

use image::{DynamicImage, ImageBuffer};

/// Bigger thumbnails than this are not read (bogus files).
const MAX_SIDE: u32 = 1024;

pub(crate) const BUCKETS: [(u32, &str); 3] = [(128, "normal"), (256, "large"), (512, "x-large")];

/// The smallest bucket that covers `size` (requests are ≤ 512).
pub(crate) fn bucket_for(size: u32) -> (u32, &'static str) {
    BUCKETS.into_iter().find(|(b, _)| *b >= size).unwrap_or(BUCKETS[2])
}

/// `file://` URI escaped like GLib's g_filename_to_uri, so the MD5 names
/// match what GNOME apps write.
pub(crate) fn uri(path: &str) -> String {
    let mut out = String::from("file://");
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || b"!$&'()*+,-./:=@_~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn file_name(uri: &str) -> String {
    format!("{:x}.png", md5::compute(uri))
}

/// What a thumbnail must say about the file it stands for.
pub(crate) struct Source<'a> {
    pub path: &'a str,
    pub mtime: i64,
    pub size: u64,
}

/// A valid cached thumbnail at least `size` big (smallest first).
pub(crate) fn lookup(root: &Path, src: &Source<'_>, size: u32) -> Option<DynamicImage> {
    let uri = uri(src.path);
    let name = file_name(&uri);
    BUCKETS
        .iter()
        .filter(|(b, _)| *b >= size)
        .find_map(|(_, dir)| decode_valid(&std::fs::read(root.join(dir).join(&name)).ok()?, &uri, src))
}

fn decode_valid(bytes: &[u8], uri: &str, src: &Source<'_>) -> Option<DynamicImage> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    reader.next_frame(&mut buf).ok()?;
    // Text chunks may also come after the image data.
    reader.finish().ok()?;
    let text = |key: &str| {
        let info = reader.info();
        info.uncompressed_latin1_text
            .iter()
            .find(|t| t.keyword == key)
            .map(|t| t.text.clone())
            .or_else(|| info.utf8_text.iter().find(|t| t.keyword == key).and_then(|t| t.get_text().ok()))
    };
    if text("Thumb::URI")? != uri || text("Thumb::MTime")?.trim() != src.mtime.to_string() {
        return None;
    }
    if text("Thumb::Size").is_some_and(|s| s.trim() != src.size.to_string()) {
        return None;
    }
    let (color, _) = reader.output_color_type();
    match color {
        png::ColorType::Grayscale => ImageBuffer::from_raw(w, h, buf).map(DynamicImage::ImageLuma8),
        png::ColorType::GrayscaleAlpha => ImageBuffer::from_raw(w, h, buf).map(DynamicImage::ImageLumaA8),
        png::ColorType::Rgb => ImageBuffer::from_raw(w, h, buf).map(DynamicImage::ImageRgb8),
        png::ColorType::Rgba => ImageBuffer::from_raw(w, h, buf).map(DynamicImage::ImageRgba8),
        png::ColorType::Indexed => None,
    }
}

/// Save `img` (already fit to the bucket) as `src`'s thumbnail. Atomic,
/// private (0600 file in a 0700 dir); failures only cost a later re-render.
pub(crate) fn store(root: &Path, bucket: &str, src: &Source<'_>, mime: &str, img: &DynamicImage) {
    if let Err(e) = try_store(root, bucket, src, mime, img) {
        tracing::debug!("thumbnail cache {}: {e}", root.display());
    }
}

fn try_store(root: &Path, bucket: &str, src: &Source<'_>, mime: &str, img: &DynamicImage) -> std::io::Result<()> {
    let dir = root.join(bucket);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&dir)?;

    let uri = uri(src.path);
    let rgba = img.to_rgba8();
    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, rgba.width(), rgba.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        for (k, v) in [
            ("Thumb::URI", uri.clone()),
            ("Thumb::MTime", src.mtime.to_string()),
            ("Thumb::Size", src.size.to_string()),
            ("Thumb::Mimetype", mime.to_string()),
            ("Software", "VPS Files".to_string()),
        ] {
            enc.add_text_chunk(k.to_string(), v).map_err(std::io::Error::other)?;
        }
        let mut w = enc.write_header().map_err(std::io::Error::other)?;
        w.write_image_data(rgba.as_raw()).map_err(std::io::Error::other)?;
        w.finish().map_err(std::io::Error::other)?;
    }

    let name = file_name(&uri);
    let tmp = dir.join(format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(&tmp)?.write_all(&png_bytes)?;
        std::fs::rename(&tmp, dir.join(&name))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn uris_and_names_match_glib() {
        assert_eq!(uri("/home/me/a b/ü.jpg"), "file:///home/me/a%20b/%C3%BC.jpg");
        assert_eq!(uri("/x/#?%;[]{}\"<>^`|\\"), "file:///x/%23%3F%25%3B%5B%5D%7B%7D%22%3C%3E%5E%60%7C%5C");
        assert_eq!(uri("/k/!$&'()*+,-.:=@_~"), "file:///k/!$&'()*+,-.:=@_~");
        // The spec's own example: md5("file:///home/jens/photos/me.png").
        assert_eq!(file_name("file:///home/jens/photos/me.png"), "c6ee772d9e49320e97ec29a7eb5b1697.png");
        assert_eq!(bucket_for(32), (128, "normal"));
        assert_eq!(bucket_for(129), (256, "large"));
        assert_eq!(bucket_for(512), (512, "x-large"));
    }

    #[test]
    fn store_then_lookup_and_validity() {
        let root = tempfile::tempdir().unwrap();
        let src = Source { path: "/photos/cat.jpg", mtime: 1_700_000_000, size: 12345 };
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(256, 192, Rgb([1, 2, 3])));
        store(root.path(), "large", &src, "image/jpeg", &img);

        let found = lookup(root.path(), &src, 200).unwrap();
        assert_eq!((found.width(), found.height()), (256, 192));
        assert!(lookup(root.path(), &src, 100).is_some(), "a bigger bucket serves small requests");
        assert!(lookup(root.path(), &src, 300).is_none(), "too small for 300");
        assert!(lookup(root.path(), &Source { mtime: 1, ..src }, 200).is_none(), "file changed");
        assert!(lookup(root.path(), &Source { size: 1, ..src }, 200).is_none(), "other size");
        assert!(lookup(root.path(), &Source { path: "/photos/dog.jpg", ..src }, 200).is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = root.path().join("large");
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
            let f = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp files left");
        }
    }
}
