//! The notification-area (tray) icon: Purlay keeps running there when its window is
//! closed, shows a red dot while something waits for the person, and offers Open,
//! Settings and Quit. A click on the icon brings the window back.

use crate::i18n::t;
use gpui_kit::*;
use std::cell::Cell;

/// What the icon or its menu asks for, handed to the app's main loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Settings,
    Quit,
}

const SIZE: u32 = 32;

pub struct Tray {
    #[cfg(windows)]
    icon: tray_icon::TrayIcon,
    base: Vec<u8>,
    shown: Cell<Option<i64>>,
}

impl Global for Tray {}

/// The logo at the tray's size, as RGBA.
fn base_icon() -> Vec<u8> {
    match image::load_from_memory(crate::assets::LOGO_PNG) {
        Ok(img) => img.resize_exact(SIZE, SIZE, image::imageops::FilterType::Lanczos3).to_rgba8().into_raw(),
        Err(_) => vec![0; (SIZE * SIZE * 4) as usize],
    }
}

/// The logo with a red dot in the lower right corner, ringed so it reads on any taskbar.
fn with_dot(base: &[u8]) -> Vec<u8> {
    let mut out = base.to_vec();
    let (cx, cy, r, ring) = (24.5f32, 24.5f32, 6.5f32, 1.6f32);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let outer = (r + ring + 0.5 - d).clamp(0.0, 1.0);
            if outer <= 0.0 {
                continue;
            }
            let inner = (r + 0.5 - d).clamp(0.0, 1.0);
            // Red inside, white ring around it, blended over the logo by coverage.
            let (cr, cg, cb) = (220.0 * inner + 255.0 * (1.0 - inner), 38.0 * inner + 255.0 * (1.0 - inner), 38.0 * inner + 255.0 * (1.0 - inner));
            let i = ((y * SIZE + x) * 4) as usize;
            let a = outer;
            out[i] = (out[i] as f32 * (1.0 - a) + cr * a) as u8;
            out[i + 1] = (out[i + 1] as f32 * (1.0 - a) + cg * a) as u8;
            out[i + 2] = (out[i + 2] as f32 * (1.0 - a) + cb * a) as u8;
            out[i + 3] = (out[i + 3] as f32 * (1.0 - a) + 255.0 * a) as u8;
        }
    }
    out
}

/// Puts the icon in the notification area; its clicks and menu choices arrive on the
/// returned receiver.
#[cfg(windows)]
pub fn install(cx: &mut App) -> Option<tokio::sync::mpsc::UnboundedReceiver<TrayCommand>> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let open = MenuItem::new(t("tray.open"), true, None);
    let settings = MenuItem::new(t("nav.settings"), true, None);
    let quit = MenuItem::new(t("tray.quit"), true, None);
    let menu = Menu::new();
    let _ = menu.append(&open);
    let _ = menu.append(&settings);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);

    let ids = (open.id().clone(), settings.id().clone(), quit.id().clone());
    let tx_menu = tx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let cmd = if e.id == ids.0 {
            TrayCommand::Show
        } else if e.id == ids.1 {
            TrayCommand::Settings
        } else if e.id == ids.2 {
            TrayCommand::Quit
        } else {
            return;
        };
        let _ = tx_menu.send(cmd);
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| match e {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => {
            let _ = tx.send(TrayCommand::Show);
        }
        _ => {}
    }));

    let base = base_icon();
    let icon = tray_icon::Icon::from_rgba(base.clone(), SIZE, SIZE).ok()?;
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Purlay")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| tracing::warn!("tray icon: {e}"))
        .ok()?;
    cx.set_global(Tray { icon: tray, base, shown: Cell::new(Some(0)) });
    Some(rx)
}

#[cfg(not(windows))]
pub fn install(_cx: &mut App) -> Option<tokio::sync::mpsc::UnboundedReceiver<TrayCommand>> {
    None
}

/// Shows how many things wait: a red dot on the icon and the count in its tooltip.
pub fn set_unread(n: i64, cx: &App) {
    let Some(tray) = cx.try_global::<Tray>() else { return };
    if tray.shown.get() == Some(n) {
        return;
    }
    tray.shown.set(Some(n));
    #[cfg(windows)]
    {
        let rgba = if n > 0 { with_dot(&tray.base) } else { tray.base.clone() };
        if let Ok(icon) = tray_icon::Icon::from_rgba(rgba, SIZE, SIZE) {
            let _ = tray.icon.set_icon(Some(icon));
        }
        let tip = if n > 0 { format!("Purlay · {}", crate::i18n::tf("tray.unread", &[&n])) } else { "Purlay".to_string() };
        let _ = tray.icon.set_tooltip(Some(tip));
    }
}

pub fn installed(cx: &App) -> bool {
    cx.try_global::<Tray>().is_some()
}

/// Hides the window (it keeps running; the tray brings it back).
pub fn hide_window(window: &Window) {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

/// Shows the window again if hidden, then brings it to the front.
pub fn show_window(window: &mut Window) {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindow, SW_SHOW};
        unsafe {
            if !IsWindowVisible(h).as_bool() {
                let _ = ShowWindow(h, SW_SHOW);
            }
        }
    }
    window.activate_window();
}

#[cfg(windows)]
fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{base_icon, with_dot, SIZE};

    #[test]
    fn the_dot_paints_only_the_corner() {
        let base = base_icon();
        assert_eq!(base.len(), (SIZE * SIZE * 4) as usize);
        let dotted = with_dot(&base);
        let px = |b: &[u8], x: u32, y: u32| {
            let i = ((y * SIZE + x) * 4) as usize;
            (b[i], b[i + 1], b[i + 2], b[i + 3])
        };
        // The centre of the dot is red and opaque; the top left is untouched.
        let c = px(&dotted, 24, 24);
        assert!(c.0 > 200 && c.1 < 60 && c.3 == 255, "{c:?}");
        assert_eq!(px(&dotted, 4, 4), px(&base, 4, 4));
    }
}
