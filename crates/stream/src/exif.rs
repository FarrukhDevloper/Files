//! Just enough of a JPEG header for thumbnails: the EXIF Orientation, the
//! embedded JPEG thumbnail (IFD1) and the frame size. Works on a prefix of
//! the file; anything that doesn't fit or parse is simply absent.

use std::ops::Range;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct JpegInfo {
    /// EXIF Orientation (1..=8).
    pub orientation: Option<u16>,
    /// Byte range of the embedded JPEG thumbnail within the prefix.
    pub thumbnail: Option<Range<usize>>,
    /// Width × height from the SOF segment.
    pub size: Option<(u32, u32)>,
    /// The only SOF is 8-bit DCT (baseline/progressive) with 1 or 3
    /// components: safe for the scaled decoder, whose metadata code panics
    /// on other shapes (and release builds abort on panic).
    pub plain_dct: bool,
}

pub(crate) fn is_jpeg(data: &[u8]) -> bool {
    data.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// Walk the segments before the image data.
pub(crate) fn parse(data: &[u8]) -> JpegInfo {
    let mut info = JpegInfo::default();
    if !is_jpeg(data) {
        return info;
    }
    let mut i = 2;
    let mut sofs = 0;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            break;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1; // fill byte
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            break; // EOI / start of scan: no more headers
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2; // no length
            continue;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        if len < 2 {
            break;
        }
        let start = i + 4;
        let end = (i + 2 + len).min(data.len());
        let payload = &data[start..end];
        match marker {
            0xE1 if payload.starts_with(b"Exif\0\0") && info.orientation.is_none() && info.thumbnail.is_none() => {
                let tiff = start + 6;
                if let Some(TiffInfo { orientation, thumbnail }) = data.get(tiff..).and_then(tiff_info) {
                    info.orientation = orientation;
                    info.thumbnail = thumbnail.and_then(|(off, len)| {
                        let start = tiff.checked_add(off)?;
                        Some(start..start.checked_add(len)?)
                    });
                }
            }
            // SOFn, except DHT (C4), JPG (C8) and DAC (CC).
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) && payload.len() >= 6 => {
                let h = u16::from_be_bytes([payload[1], payload[2]]);
                let w = u16::from_be_bytes([payload[3], payload[4]]);
                let first = sofs == 0;
                sofs += 1;
                if first && w > 0 && h > 0 {
                    info.size = Some((u32::from(w), u32::from(h)));
                    // C0 baseline, C1 extended, C2 progressive (Huffman).
                    info.plain_dct = matches!(marker, 0xC0..=0xC2) && payload[0] == 8 && matches!(payload[5], 1 | 3);
                }
            }
            _ => {}
        }
        i += 2 + len;
    }
    if sofs > 1 {
        info.plain_dct = false;
    }
    // The embedded image must be there completely and be a JPEG itself.
    if let Some(r) = &info.thumbnail
        && (r.end > data.len() || !is_jpeg(&data[r.clone()]))
    {
        info.thumbnail = None;
    }
    info
}

/// (tag, type, offset of the value field) per IFD entry.
type Entries = Vec<(u16, u16, usize)>;

struct TiffInfo {
    orientation: Option<u16>,
    /// (offset, length) relative to the TIFF header.
    thumbnail: Option<(usize, usize)>,
}

struct Tiff<'a> {
    data: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at + 4)?.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    /// (tag, type, value field offset) of each entry, and the next IFD.
    fn ifd(&self, at: usize) -> Option<(Entries, u32)> {
        let count = usize::from(self.u16(at)?);
        if count > 512 {
            return None;
        }
        let entries = (0..count)
            .map(|k| {
                let e = at + 2 + k * 12;
                Some((self.u16(e)?, self.u16(e + 2)?, e + 8))
            })
            .collect::<Option<Vec<_>>>()?;
        Some((entries, self.u32(at + 2 + count * 12).unwrap_or(0)))
    }

    /// A SHORT or LONG value stored inline.
    fn number(&self, ty: u16, at: usize) -> Option<u32> {
        match ty {
            3 => self.u16(at).map(u32::from),
            4 => self.u32(at),
            _ => None,
        }
    }
}

/// Orientation from IFD0 and the IFD1 thumbnail; `data` starts at the TIFF
/// header.
fn tiff_info(data: &[u8]) -> Option<TiffInfo> {
    let le = match data.get(..4)? {
        [b'I', b'I', 42, 0] => true,
        [b'M', b'M', 0, 42] => false,
        _ => return None,
    };
    let t = Tiff { data, le };
    let (ifd0, next) = t.ifd(t.u32(4)? as usize)?;
    let orientation = ifd0
        .iter()
        .find(|(tag, ..)| *tag == 0x0112)
        .and_then(|&(_, ty, at)| t.number(ty, at))
        .and_then(|v| u16::try_from(v).ok())
        .filter(|v| (1..=8).contains(v));
    let thumbnail = (next != 0).then(|| t.ifd(next as usize)).flatten().and_then(|(ifd1, _)| {
        let find = |tag| ifd1.iter().find(|e| e.0 == tag).and_then(|&(_, ty, at)| t.number(ty, at));
        let (off, len) = (find(0x0201)? as usize, find(0x0202)? as usize);
        (len > 0).then_some((off, len))
    });
    Some(TiffInfo { orientation, thumbnail })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A TIFF block: IFD0 with Orientation, IFD1 pointing at `thumb`.
    fn tiff(le: bool, orientation: u16, thumb: &[u8]) -> Vec<u8> {
        let u16b = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let mut t = Vec::new();
        t.extend_from_slice(if le { b"II" } else { b"MM" });
        t.extend_from_slice(&u16b(42));
        t.extend_from_slice(&u32b(8));
        // IFD0 at 8: one entry, next IFD at 26.
        t.extend_from_slice(&u16b(1));
        t.extend_from_slice(&u16b(0x0112));
        t.extend_from_slice(&u16b(3));
        t.extend_from_slice(&u32b(1));
        t.extend_from_slice(&u16b(orientation));
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&u32b(26));
        // IFD1 at 26: offset + length, thumbnail right after (at 56).
        t.extend_from_slice(&u16b(2));
        for (tag, v) in [(0x0201u16, 56u32), (0x0202, thumb.len() as u32)] {
            t.extend_from_slice(&u16b(tag));
            t.extend_from_slice(&u16b(4));
            t.extend_from_slice(&u32b(1));
            t.extend_from_slice(&u32b(v));
        }
        t.extend_from_slice(&u32b(0));
        assert_eq!(t.len(), 56);
        t.extend_from_slice(thumb);
        t
    }

    /// SOI, APP1 (Exif), SOF0 (w × h), SOS: a JPEG header as cameras write it.
    pub(crate) fn jpeg_with_exif(le: bool, orientation: u16, thumb: &[u8], w: u16, h: u16) -> Vec<u8> {
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(tiff(le, orientation, thumb));
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE1];
        j.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        j.extend(app1);
        j.extend_from_slice(&[0xFF, 0xC0, 0, 11, 8]);
        j.extend_from_slice(&h.to_be_bytes());
        j.extend_from_slice(&w.to_be_bytes());
        j.extend_from_slice(&[1, 1, 0x11, 0]);
        j.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
        j
    }

    #[test]
    fn finds_orientation_thumbnail_and_size() {
        let thumb = [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 0xFF, 0xD9];
        for le in [true, false] {
            let j = jpeg_with_exif(le, 6, &thumb, 4000, 3000);
            let info = parse(&j);
            assert_eq!(info.orientation, Some(6));
            assert_eq!(info.size, Some((4000, 3000)));
            assert!(info.plain_dct);
            assert_eq!(&j[info.thumbnail.unwrap()], &thumb);
        }
    }

    #[test]
    fn only_plain_dct_frames_go_to_the_scaled_decoder() {
        let base = jpeg_with_exif(true, 1, &[], 10, 10);
        let sof = base.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        let with = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut j = base.clone();
            f(&mut j);
            parse(&j).plain_dct
        };
        assert!(with(&|_| {}));
        assert!(!with(&|j| j[sof + 9] = 2), "2 components");
        assert!(!with(&|j| j[sof + 4] = 12), "12-bit");
        assert!(!with(&|j| j[sof + 1] = 0xC3), "lossless");
        assert!(!with(&|j| {
            let again = j[sof..sof + 13].to_vec();
            j.splice(sof..sof, again);
        }), "two frames");
    }

    #[test]
    fn partial_or_broken_input_is_safe() {
        let thumb = [0xFF, 0xD8, 0xFF, 0xE0, 9, 9, 0xFF, 0xD9];
        let j = jpeg_with_exif(true, 3, &thumb, 10, 10);
        for cut in 0..j.len() {
            let info = parse(&j[..cut]);
            if let Some(r) = info.thumbnail {
                assert!(r.end <= cut);
            }
        }
        // Thumbnail that isn't a JPEG is ignored; garbage is no JPEG.
        let j = jpeg_with_exif(true, 3, b"not a jpeg", 10, 10);
        assert_eq!(parse(&j).thumbnail, None);
        assert_eq!(parse(b"\x89PNG...."), JpegInfo::default());
        assert_eq!(parse(&[0xFF, 0xD8, 0xFF, 0xE1, 0, 0]), JpegInfo::default());
    }
}
