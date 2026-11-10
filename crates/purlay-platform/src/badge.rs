//! The count on Purlay's taskbar button: a red disc with a white number, laid over the
//! icon with `ITaskbarList3::SetOverlayIcon` (the way unpackaged apps badge the taskbar).
//!
//! The digits are drawn here as rounded strokes, so the badge needs no font and looks the
//! same on every machine; 10 and more read "9+".

use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// The badge's text for a count: `1`…`9`, then `9+`.
pub fn label(count: u32) -> String {
    if count > 9 { "9+".into() } else { count.to_string() }
}

/// One glyph in a 6×10 box (x right, y down), as strokes.
fn glyph(c: char) -> Option<Path> {
    let mut p = PathBuilder::new();
    match c {
        '0' => {
            p.push_oval(Rect::from_xywh(0.3, 0.0, 5.4, 10.0)?);
        }
        '1' => {
            p.move_to(1.2, 2.2);
            p.line_to(3.6, 0.0);
            p.line_to(3.6, 10.0);
        }
        '2' => {
            p.move_to(0.4, 2.6);
            p.cubic_to(0.4, 0.9, 1.6, 0.0, 3.0, 0.0);
            p.cubic_to(4.6, 0.0, 5.6, 1.1, 5.6, 2.6);
            p.cubic_to(5.6, 4.2, 4.4, 5.2, 3.2, 6.3);
            p.line_to(0.3, 10.0);
            p.line_to(5.8, 10.0);
        }
        '3' => {
            p.move_to(0.4, 1.3);
            p.cubic_to(1.0, 0.4, 1.9, 0.0, 3.0, 0.0);
            p.cubic_to(4.6, 0.0, 5.5, 1.1, 5.5, 2.5);
            p.cubic_to(5.5, 4.0, 4.4, 4.9, 2.7, 4.9);
            p.cubic_to(4.6, 4.9, 5.8, 6.0, 5.8, 7.5);
            p.cubic_to(5.8, 9.1, 4.6, 10.0, 3.0, 10.0);
            p.cubic_to(1.8, 10.0, 0.8, 9.6, 0.2, 8.8);
        }
        '4' => {
            p.move_to(4.4, 10.0);
            p.line_to(4.4, 0.0);
            p.line_to(0.1, 7.0);
            p.line_to(6.0, 7.0);
        }
        '5' => {
            p.move_to(5.4, 0.0);
            p.line_to(1.0, 0.0);
            p.line_to(0.6, 4.6);
            p.cubic_to(1.2, 4.1, 2.1, 3.8, 3.0, 3.8);
            p.cubic_to(4.8, 3.8, 5.8, 5.0, 5.8, 6.8);
            p.cubic_to(5.8, 8.7, 4.6, 10.0, 2.9, 10.0);
            p.cubic_to(1.7, 10.0, 0.8, 9.6, 0.2, 8.9);
        }
        '6' => {
            p.move_to(5.0, 0.5);
            p.cubic_to(4.5, 0.2, 3.8, 0.0, 3.2, 0.0);
            p.cubic_to(1.2, 0.0, 0.2, 2.2, 0.2, 6.8);
            p.push_oval(Rect::from_xywh(0.2, 3.9, 5.6, 6.1)?);
        }
        '7' => {
            p.move_to(0.2, 0.0);
            p.line_to(5.8, 0.0);
            p.line_to(2.2, 10.0);
        }
        '8' => {
            p.push_oval(Rect::from_xywh(0.7, 0.0, 4.6, 4.7)?);
            p.push_oval(Rect::from_xywh(0.2, 4.7, 5.6, 5.3)?);
        }
        '9' => {
            p.push_oval(Rect::from_xywh(0.2, 0.0, 5.6, 6.1)?);
            p.move_to(5.8, 3.2);
            p.cubic_to(5.8, 7.8, 4.8, 10.0, 2.8, 10.0);
            p.cubic_to(2.2, 10.0, 1.5, 9.8, 1.0, 9.5);
        }
        '+' => {
            p.move_to(0.6, 5.0);
            p.line_to(5.4, 5.0);
            p.move_to(3.0, 2.6);
            p.line_to(3.0, 7.4);
        }
        _ => return None,
    }
    p.finish()
}

/// The badge as straight (not premultiplied) RGBA, `size`×`size`.
pub fn render(count: u32, size: u32) -> Vec<u8> {
    let Some(mut pix) = Pixmap::new(size, size) else { return vec![] };
    let s = size as f32;
    let mut paint = Paint { anti_alias: true, ..Paint::default() };

    // The disc: Windows' red, with a thin white ring so it stands off any icon.
    paint.set_color_rgba8(255, 255, 255, 255);
    if let Some(c) = PathBuilder::from_circle(s / 2.0, s / 2.0, s / 2.0) {
        pix.fill_path(&c, &paint, FillRule::Winding, Transform::identity(), None);
    }
    paint.set_color_rgba8(209, 52, 56, 255);
    if let Some(c) = PathBuilder::from_circle(s / 2.0, s / 2.0, s / 2.0 - s * 0.07) {
        pix.fill_path(&c, &paint, FillRule::Winding, Transform::identity(), None);
    }

    // The digits, centred; two characters are set narrower to fit the disc.
    let text = label(count);
    let n = text.chars().count() as f32;
    let glyph_h = s * if n > 1.0 { 0.40 } else { 0.48 };
    let k = glyph_h / 10.0;
    let advance = 6.0 * k + s * 0.06;
    let total = advance * n - s * 0.06;
    let x0 = (s - total) / 2.0;
    let y0 = (s - glyph_h) / 2.0;
    paint.set_color_rgba8(255, 255, 255, 255);
    let ink = s * if n > 1.0 { 0.085 } else { 0.10 };
    let stroke = Stroke { width: ink / k.max(0.01), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
    for (i, ch) in text.chars().enumerate() {
        if let Some(g) = glyph(ch) {
            let t = Transform::from_row(k, 0.0, 0.0, k, x0 + advance * i as f32, y0);
            pix.stroke_path(&g, &paint, &stroke, t, None);
        }
    }

    // tiny-skia keeps premultiplied colour; Windows icons want it straight.
    let mut out = pix.take();
    for px in out.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

/// Shows `count` on the taskbar button of `hwnd` (0 clears it). `describe` is read out by
/// screen readers. Must run on the window's thread.
#[cfg(windows)]
pub fn set(hwnd: isize, count: u32, describe: &str) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetSystemMetrics, HICON, SM_CXSMICON};
    if hwnd == 0 {
        tracing::debug!("badge: no window yet");
        return;
    }
    let hwnd = HWND(hwnd as *mut _);
    unsafe {
        let list = match CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!("badge: no taskbar list: {e}");
                return;
            }
        };
        if let Err(e) = list.HrInit() {
            tracing::debug!("badge: taskbar list init: {e}");
            return;
        }
        if count == 0 {
            let _ = list.SetOverlayIcon(hwnd, HICON::default(), &HSTRING::new());
            return;
        }
        // The overlay is drawn at the small-icon size (16 at 100 %, 24 at 150 %…).
        let size = GetSystemMetrics(SM_CXSMICON).clamp(16, 64) as u32;
        let Some(icon) = icon_from_rgba(&render(count, size), size) else { return };
        let r = list.SetOverlayIcon(hwnd, icon, &HSTRING::from(describe));
        tracing::debug!("badge: {count} on {hwnd:?}: {r:?}");
        let _ = DestroyIcon(icon);
    }
}

#[cfg(windows)]
fn icon_from_rgba(rgba: &[u8], size: u32) -> Option<windows::Win32::UI::WindowsAndMessaging::HICON> {
    use windows::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};
    unsafe {
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
        let dc = GetDC(None);
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        ReleaseDC(None, dc);
        let color = color?;
        // RGBA → BGRA.
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, rgba.len());
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
            *d = [s[2], s[1], s[0], s[3]];
        }
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

#[cfg(not(windows))]
pub fn set(_hwnd: isize, _count: u32, _describe: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_stop_at_nine_plus() {
        assert_eq!(label(1), "1");
        assert_eq!(label(9), "9");
        assert_eq!(label(10), "9+");
        assert_eq!(label(250), "9+");
    }

    #[test]
    fn every_badge_draws_a_red_disc_with_white_ink() {
        for n in 1..=10 {
            for size in [16u32, 24, 32] {
                let px = render(n, size);
                assert_eq!(px.len(), (size * size * 4) as usize);
                let at = |x: u32, y: u32| {
                    let i = ((y * size + x) * 4) as usize;
                    (px[i], px[i + 1], px[i + 2], px[i + 3])
                };
                // Corners are clear, the rim is red, and some ink is white.
                assert_eq!(at(0, 0).3, 0, "corner n={n} size={size}");
                let rim = at(size / 2, size / 6);
                assert!(rim.0 > 180 && rim.1 < 90, "rim n={n} size={size}: {rim:?}");
                let white = px.as_chunks::<4>().0.iter().filter(|p| p[0] > 230 && p[1] > 230 && p[2] > 230 && p[3] > 230).count();
                assert!(white > (size * size / 30) as usize, "ink n={n} size={size}: {white}");
            }
        }
    }

    /// Writes the badges as PNG files for a look (`cargo test -p purlay-platform -- --ignored`).
    #[test]
    #[ignore]
    fn dump() {
        for n in [1, 2, 3, 4, 5, 6, 7, 8, 9, 12] {
            let px = render(n, 96);
            let mut p = Pixmap::new(96, 96).unwrap();
            for (d, s) in p.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(px.as_chunks::<4>().0) {
                let a = s[3] as u32;
                *d = [(s[0] as u32 * a / 255) as u8, (s[1] as u32 * a / 255) as u8, (s[2] as u32 * a / 255) as u8, s[3]];
            }
            p.save_png(std::env::temp_dir().join(format!("purlay-badge-{n}.png"))).unwrap();
        }
    }
}
