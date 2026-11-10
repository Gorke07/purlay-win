//! Just enough image handling for uploads and seed data: a PNG encoder (stored deflate
//! blocks, no compression), and reading the size and type of PNG, JPEG, GIF and WebP.

use purlay_contract::models::AttachmentKind;

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xffff_ffffu32;
    for b in data {
        crc = table[((crc ^ *b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for x in data {
        a = (a + *x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut c = Vec::with_capacity(4 + data.len());
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    out.extend_from_slice(&c);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

/// An RGB PNG of `width` by `height`, each pixel from `pixel(x, y)`.
pub fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width * 3 + 1) as usize * height as usize);
    for y in 0..height {
        raw.push(0); // filter: none
        for x in 0..width {
            raw.extend_from_slice(&pixel(x, y));
        }
    }
    // zlib stream of stored blocks
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(block) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, RGB
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The seed's screenshot: a gradient with a few panels, 480 by 270.
pub fn demo_png() -> Vec<u8> {
    png(480, 270, |x, y| {
        let in_box = |x0: u32, y0: u32, x1: u32, y1: u32| x >= x0 && x < x1 && y >= y0 && y < y1;
        if in_box(0, 0, 480, 28) {
            return [32, 36, 48];
        }
        if in_box(0, 28, 120, 270) {
            return [44, 50, 66];
        }
        if in_box(140, 48, 460, 90) || in_box(140, 110, 400, 150) || in_box(140, 170, 440, 210) {
            return [236, 240, 247];
        }
        let r = 90 + (x * 100 / 480) as u8;
        let g = 120 + (y * 80 / 270) as u8;
        [r, g, 200]
    })
}

/// What a file is, from its first bytes and its name: (mime type, kind, width, height).
pub fn classify(bytes: &[u8], name: &str) -> (String, AttachmentKind, Option<i64>, Option<i64>) {
    if let Some((mime, w, h)) = image_info(bytes) {
        return (mime.to_string(), AttachmentKind::Image, w.map(i64::from), h.map(i64::from));
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let (mime, kind) = match ext.as_str() {
        "mp4" | "m4v" => ("video/mp4", AttachmentKind::Video),
        "mov" => ("video/quicktime", AttachmentKind::Video),
        "webm" => ("video/webm", AttachmentKind::Video),
        "mp3" => ("audio/mpeg", AttachmentKind::Audio),
        "wav" => ("audio/wav", AttachmentKind::Audio),
        "ogg" | "oga" => ("audio/ogg", AttachmentKind::Audio),
        "m4a" => ("audio/mp4", AttachmentKind::Audio),
        "txt" | "log" | "md" => ("text/plain", AttachmentKind::File),
        "csv" => ("text/csv", AttachmentKind::File),
        "json" => ("application/json", AttachmentKind::File),
        "pdf" => ("application/pdf", AttachmentKind::File),
        "zip" => ("application/zip", AttachmentKind::File),
        _ => ("application/octet-stream", AttachmentKind::File),
    };
    (mime.to_string(), kind, None, None)
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(((*b.get(i)? as u32) << 8) | *b.get(i + 1)? as u32)
}

fn le16(b: &[u8], i: usize) -> Option<u32> {
    Some(((*b.get(i + 1)? as u32) << 8) | *b.get(i)? as u32)
}

fn le24(b: &[u8], i: usize) -> Option<u32> {
    Some(((*b.get(i + 2)? as u32) << 16) | ((*b.get(i + 1)? as u32) << 8) | *b.get(i)? as u32)
}

/// (mime, width, height) of a PNG, JPEG, GIF or WebP; the size when it can be read.
pub fn image_info(b: &[u8]) -> Option<(&'static str, Option<u32>, Option<u32>)> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        let w = b.get(16..20).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
        let h = b.get(20..24).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
        return Some(("image/png", w, h));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(("image/gif", le16(b, 6), le16(b, 8)));
    }
    if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        let size = match b.get(12..16) {
            Some(b"VP8X") => le24(b, 24).zip(le24(b, 27)).map(|(w, h)| (w + 1, h + 1)),
            Some(b"VP8L") => {
                let bits = b.get(21..25).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
                bits.map(|v| ((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1))
            }
            Some(b"VP8 ") => le16(b, 26).zip(le16(b, 28)).map(|(w, h)| (w & 0x3fff, h & 0x3fff)),
            _ => None,
        };
        return Some(("image/webp", size.map(|s| s.0), size.map(|s| s.1)));
    }
    if b.starts_with(&[0xff, 0xd8]) {
        // Walk the segments to the first start-of-frame.
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xff {
                i += 1;
                continue;
            }
            let marker = b[i + 1];
            if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
                i += 2;
                continue;
            }
            let len = be16(b, i + 2)? as usize;
            let sof = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
            if sof {
                return Some(("image/jpeg", be16(b, i + 7), be16(b, i + 5)));
            }
            i += 2 + len;
        }
        return Some(("image/jpeg", None, None));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_reads_back() {
        let p = png(3, 2, |_, _| [1, 2, 3]);
        assert_eq!(image_info(&p), Some(("image/png", Some(3), Some(2))));
        let (mime, kind, w, h) = classify(&demo_png(), "shot.png");
        assert_eq!((mime.as_str(), kind, w, h), ("image/png", AttachmentKind::Image, Some(480), Some(270)));
        assert_eq!(classify(b"hello", "notes.txt").1, AttachmentKind::File);
    }
}
