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
    /// Clears a chosen away.
    Online,
    Away,
    /// Deep Work for this many minutes (-1: until tomorrow 09:00).
    DeepWork(i64),
    EndDeepWork,
}

const SIZE: u32 = 32;

pub struct Tray {
    #[cfg(windows)]
    icon: tray_icon::TrayIcon,
    #[cfg(windows)]
    status: StatusItems,
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

/// The menu items that show the person's status, updated as it changes.
#[cfg(windows)]
struct StatusItems {
    online: tray_icon::menu::CheckMenuItem,
    away: tray_icon::menu::CheckMenuItem,
    deep: tray_icon::menu::Submenu,
    deep_end: tray_icon::menu::MenuItem,
    shown: Cell<Option<(bool, Option<i64>)>>,
}

/// Puts the icon in the notification area; its clicks and menu choices arrive on the
/// returned receiver.
#[cfg(windows)]
pub fn install(cx: &mut App) -> Option<tokio::sync::mpsc::UnboundedReceiver<TrayCommand>> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    use tray_icon::menu::{CheckMenuItem, Submenu};
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let open = MenuItem::new(t("tray.open"), true, None);
    let online = CheckMenuItem::new(t("presence.online"), true, true, None);
    let away = CheckMenuItem::new(t("presence.away"), true, false, None);
    let deep = Submenu::new(t("deep.title"), true);
    let mut commands: Vec<(tray_icon::menu::MenuId, TrayCommand)> = vec![];
    for (key, mins) in [("deep.30m", 30i64), ("deep.1h", 60), ("deep.2h", 120), ("deep.tomorrow", -1)] {
        let item = MenuItem::new(t(key), true, None);
        commands.push((item.id().clone(), TrayCommand::DeepWork(mins)));
        let _ = deep.append(&item);
    }
    let deep_end = MenuItem::new(t("deep.end"), false, None);
    let _ = deep.append(&PredefinedMenuItem::separator());
    let _ = deep.append(&deep_end);
    let settings = MenuItem::new(t("nav.settings"), true, None);
    let quit = MenuItem::new(t("tray.quit"), true, None);
    let menu = Menu::new();
    let _ = menu.append(&open);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&online);
    let _ = menu.append(&away);
    let _ = menu.append(&deep);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&settings);
    let _ = menu.append(&quit);

    commands.extend([
        (open.id().clone(), TrayCommand::Show),
        (online.id().clone(), TrayCommand::Online),
        (away.id().clone(), TrayCommand::Away),
        (deep_end.id().clone(), TrayCommand::EndDeepWork),
        (settings.id().clone(), TrayCommand::Settings),
        (quit.id().clone(), TrayCommand::Quit),
    ]);
    let tx_menu = tx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Some((_, cmd)) = commands.iter().find(|(id, _)| *id == e.id) {
            let _ = tx_menu.send(*cmd);
        }
    }));
    let status = StatusItems { online, away, deep, deep_end, shown: Cell::new(None) };
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
    cx.set_global(Tray { icon: tray, status, base, shown: Cell::new(Some(0)) });
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

/// Ticks the status the person is in (online or away) and names Deep Work's end.
pub fn set_status(away: bool, deep_until: Option<i64>, cx: &App) {
    let Some(tray) = cx.try_global::<Tray>() else { return };
    #[cfg(windows)]
    {
        let st = &tray.status;
        if st.shown.get() == Some((away, deep_until)) {
            return;
        }
        st.shown.set(Some((away, deep_until)));
        st.online.set_checked(!away);
        st.away.set_checked(away);
        st.deep_end.set_enabled(deep_until.is_some());
        st.deep.set_text(match deep_until {
            Some(u) => format!("{} · {}", t("deep.title"), crate::sidebar::mute_end_text(u)),
            None => t("deep.title").to_string(),
        });
    }
    #[cfg(not(windows))]
    let _ = (tray, away, deep_until);
}

/// The workspace the person last used on this server (Deep Work is per workspace).
pub fn current_workspace(app: &crate::state::AppModel) -> Option<purlay_core::contract::Uuid> {
    let s = app.session()?;
    let prefs = app.prefs.get();
    let last = prefs.servers.iter().find(|e| Some(&e.address) == prefs.current_server.as_ref()).and_then(|e| e.last_workspace.clone());
    let store = s.store().read_recursive();
    let me = store.me.as_ref()?;
    last.and_then(|w| me.memberships.iter().find(|m| m.workspace.id.as_str() == w).map(|m| m.workspace.id.clone())).or_else(|| me.memberships.first().map(|m| m.workspace.id.clone()))
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
pub fn show_window(window: &mut Window, cx: &App) {
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
    refresh_badge_soon(cx);
}

/// Lays the badge on again once Windows has (re)made the taskbar button, which happens a
/// moment after the window is shown; the overlay set before that is dropped.
pub fn refresh_badge_soon(cx: &App) {
    for ms in [400u64, 1500, 4000] {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(ms)).await;
            refresh_badge();
        })
        .detach();
    }
}

#[cfg(windows)]
fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

// --- the taskbar badge --------------------------------------------------------------

use std::sync::atomic::{AtomicI64, AtomicIsize, Ordering};

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static BADGE: AtomicI64 = AtomicI64::new(0);

/// Remembers the main window, whose taskbar button carries the badge.
pub fn set_main_window(window: &Window) {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        MAIN_HWND.store(h.0 as isize, Ordering::Relaxed);
    }
}

/// Shows `n` on the taskbar button (nothing when 0 or turned off in Settings).
pub fn set_badge(n: i64, enabled: bool) {
    let v = if enabled { n.max(0) } else { 0 };
    if BADGE.swap(v, Ordering::Relaxed) != v {
        apply_badge(v);
    }
}

/// Lays the badge on again: Windows drops it when the taskbar button is made anew
/// (the window shown after hiding, Explorer restarted).
pub fn refresh_badge() {
    apply_badge(BADGE.load(Ordering::Relaxed));
}

fn apply_badge(n: i64) {
    let describe = if n > 0 { crate::i18n::tf("tray.unread", &[&n]) } else { String::new() };
    purlay_platform::badge::set(MAIN_HWND.load(Ordering::Relaxed), n as u32, &describe);
}


/// Brings the waiting count to every place outside the page: the window title, the
/// tray icon and the taskbar badge. Runs from the app's change observer, not from
/// drawing, so it keeps up while the window is minimized or hidden.
pub fn sync_attention(app: &Entity<crate::state::AppModel>, cx: &mut App) {
    let model = app.read(cx);
    let ready = matches!(model.phase, crate::state::Phase::Ready(_));
    let n = if ready { model.session().map(|s| s.store().read_recursive().attention_count()).unwrap_or(0) } else { 0 };
    let show_badge = !model.prefs.get().hide_taskbar_badge;
    let (away, deep_until) = match (model.session(), current_workspace(model)) {
        (Some(s), ws) => {
            let store = s.store().read_recursive();
            let away = store.me.as_ref().is_some_and(|m| m.away_chosen);
            let now = chrono::Local::now().timestamp_millis();
            let deep = ws.and_then(|w| store.membership(&w).and_then(|m| m.deep_work_until.clone())).and_then(|u| u.unix_ms()).filter(|ms| *ms > now);
            (away, deep)
        }
        _ => (false, None),
    };
    set_unread(n, cx);
    set_badge(n, show_badge);
    set_status(away, deep_until, cx);
    let title = if n > 0 { format!("Purlay ({n})") } else { "Purlay".to_string() };
    if let Some(main) = cx.try_global::<crate::banner::MainWindow>().map(|m| m.0) {
        let _ = main.update(cx, |_, w, _| w.set_window_title(&title));
    }
}


/// Keeps a window above all others (or lets it go back among them).
pub fn set_topmost(window: &Window, on: bool) {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE};
        unsafe {
            let _ = SetWindowPos(h, Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
    #[cfg(not(windows))]
    let _ = (window, on);
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
