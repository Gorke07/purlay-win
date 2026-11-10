//! The only place Purlay's tokens become GPUI colors and the gpui-kit theme.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use purlay_tokens::{Palette, Rgba as TokenRgba, DARK, LIGHT};

pub fn hsla(c: TokenRgba) -> Hsla {
    Hsla::from(gpui_kit::Rgba { r: c.r, g: c.g, b: c.b, a: c.a })
}

/// The palette in use, with the derived values views need.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Tokens {
    pub p: &'static Palette,
    pub dark: bool,
    pub sans: &'static str,
    pub mono: &'static str,
}

impl Global for Tokens {}

impl Tokens {
    pub fn get(cx: &App) -> Tokens {
        *cx.global::<Tokens>()
    }
}

/// Shorthand: `c(cx).ink` etc. as `Hsla`.
#[allow(dead_code)]
pub struct C {
    pub surface_000: Hsla,
    pub surface_050: Hsla,
    pub surface_100: Hsla,
    pub surface_200: Hsla,
    pub code_bg: Hsla,
    pub border_subtle: Hsla,
    pub border_strong: Hsla,
    pub ink: Hsla,
    pub ink_muted: Hsla,
    pub ink_subtle: Hsla,
    pub nav_rail: Hsla,
    pub nav_surface: Hsla,
    pub nav_hover: Hsla,
    pub nav_selected: Hsla,
    pub nav_border: Hsla,
    pub nav_ink: Hsla,
    pub nav_ink_muted: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_text: Hsla,
    pub on_accent: Hsla,
    pub accent_subtle: Hsla,
    pub focus_ring: Hsla,
    pub composer_edge: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub on_status: Hsla,
    pub danger_text: Hsla,
    pub warning_text: Hsla,
    pub success_text: Hsla,
    pub danger_subtle: Hsla,
    pub warning_subtle: Hsla,
    pub success_subtle: Hsla,
    pub r#match: Hsla,
    pub scrim: Hsla,
    pub syntax_keyword: Hsla,
    pub syntax_string: Hsla,
    pub syntax_number: Hsla,
    pub syntax_comment: Hsla,
    pub syntax_function: Hsla,
    pub syntax_type: Hsla,
    pub syntax_operator: Hsla,
}

pub fn c(cx: &App) -> C {
    let p = Tokens::get(cx).p;
    C {
        surface_000: hsla(p.surface_000),
        surface_050: hsla(p.surface_050),
        surface_100: hsla(p.surface_100),
        surface_200: hsla(p.surface_200),
        code_bg: hsla(p.code_bg),
        border_subtle: hsla(p.border_subtle),
        border_strong: hsla(p.border_strong),
        ink: hsla(p.ink),
        ink_muted: hsla(p.ink_muted),
        ink_subtle: hsla(p.ink_subtle),
        nav_rail: hsla(p.nav_rail),
        nav_surface: hsla(p.nav_surface),
        nav_hover: hsla(p.nav_hover),
        nav_selected: hsla(p.nav_selected),
        nav_border: hsla(p.nav_border),
        nav_ink: hsla(p.nav_ink),
        nav_ink_muted: hsla(p.nav_ink_muted),
        accent: hsla(p.accent),
        accent_hover: hsla(p.accent_hover),
        accent_text: hsla(p.accent_text),
        on_accent: hsla(p.on_accent),
        accent_subtle: hsla(p.accent_subtle),
        focus_ring: hsla(p.focus_ring),
        composer_edge: hsla(p.composer_edge),
        danger: hsla(p.danger),
        warning: hsla(p.warning),
        success: hsla(p.success),
        on_status: hsla(p.on_status),
        danger_text: hsla(p.danger_text),
        warning_text: hsla(p.warning_text),
        success_text: hsla(p.success_text),
        danger_subtle: hsla(p.danger_subtle),
        warning_subtle: hsla(p.warning_subtle),
        success_subtle: hsla(p.success_subtle),
        r#match: hsla(p.r#match),
        scrim: hsla(p.scrim),
        syntax_keyword: hsla(p.syntax_keyword),
        syntax_string: hsla(p.syntax_string),
        syntax_number: hsla(p.syntax_number),
        syntax_comment: hsla(p.syntax_comment),
        syntax_function: hsla(p.syntax_function),
        syntax_type: hsla(p.syntax_type),
        syntax_operator: hsla(p.syntax_operator),
    }
}

/// The sidebar's ground, hover and selected tones. In the dark themes the sidebar sits a
/// step lighter than the conversation (as on the Mac), so its rows lift with ink washes.
pub fn sidebar_tones(cx: &App) -> (Hsla, Hsla, Hsla) {
    let k = c(cx);
    if Tokens::get(cx).dark {
        (k.surface_200, k.ink.opacity(0.06), k.ink.opacity(0.11))
    } else {
        (k.nav_surface, k.nav_hover, k.nav_selected)
    }
}

pub fn avatar_bg(cx: &App, key: &str) -> Hsla {
    hsla(purlay_tokens::avatar_color(Tokens::get(cx).p, key))
}

fn font_installed(file: &str) -> bool {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    std::path::Path::new(&windir).join("Fonts").join(file).exists()
        || std::env::var("LOCALAPPDATA").is_ok_and(|l| std::path::Path::new(&l).join("Microsoft\\Windows\\Fonts").join(file).exists())
}

fn pick_fonts() -> (&'static str, &'static str) {
    let sans = if font_installed("SegUIVar.ttf") { "Segoe UI Variable Text" } else { "Segoe UI" };
    let mono = if font_installed("CascadiaMono.ttf") {
        "Cascadia Mono"
    } else if font_installed("CascadiaCode.ttf") {
        "Cascadia Code"
    } else {
        "Consolas"
    };
    (sans, mono)
}

/// Applies the light or dark Purlay palette to the app and to gpui-kit's components.
pub fn apply(dark: bool, cx: &mut App) {
    let (sans, mono) = pick_fonts();
    let p: &'static Palette = if dark { &DARK } else { &LIGHT };
    cx.set_global(Tokens { p, dark, sans, mono });
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    let k = c(cx);
    let selection = hsla(p.accent.with_alpha(if dark { 0.45 } else { 0.25 }));
    Theme::update(cx, |t| {
        t.font_family = sans.into();
        t.font_size = px(purlay_tokens::typography::BODY.size);
        t.mono_font_family = mono.into();
        t.mono_font_size = px(purlay_tokens::typography::CODE.size);
        t.radius = px(purlay_tokens::radius::MD);
        t.radius_lg = px(purlay_tokens::radius::RADIUS_2XL);
        t.shadow = true;

        let col = &mut t.colors;
        col.background = k.surface_100;
        col.foreground = k.ink;
        col.border = k.border_subtle;
        col.input = k.border_strong;
        col.ring = k.focus_ring;
        col.caret = k.ink;
        col.selection = selection;
        col.primary = k.accent;
        col.primary_hover = k.accent_hover;
        col.primary_active = k.accent_hover;
        col.primary_foreground = k.on_accent;
        col.button_primary = k.accent;
        col.button_primary_hover = k.accent_hover;
        col.button_primary_active = k.accent_hover;
        col.button_primary_foreground = k.on_accent;
        col.secondary = k.surface_200;
        col.secondary_hover = k.surface_050;
        col.secondary_active = k.surface_200;
        col.secondary_foreground = k.ink;
        col.muted = k.surface_200;
        col.muted_foreground = k.ink_muted;
        col.accent = k.surface_200;
        col.accent_foreground = k.ink;
        col.popover = k.surface_100;
        col.popover_foreground = k.ink;
        col.list = k.surface_100;
        col.list_hover = k.surface_050;
        col.list_active = k.accent_subtle;
        col.list_active_border = k.accent;
        col.link = k.accent_text;
        col.link_hover = k.accent_hover;
        col.link_active = k.accent_hover;
        col.danger = k.danger;
        col.danger_foreground = k.on_accent;
        col.danger_hover = k.danger_text;
        col.danger_active = k.danger_text;
        col.button_danger = k.danger;
        col.button_danger_hover = k.danger_text;
        col.button_danger_active = k.danger_text;
        col.button_danger_foreground = k.on_accent;
        col.success = k.success;
        col.success_foreground = k.on_status;
        col.warning = k.warning;
        col.warning_foreground = k.on_status;
        col.info = k.accent;
        col.info_foreground = k.on_accent;
        col.sidebar = k.nav_surface;
        col.sidebar_foreground = k.nav_ink;
        col.sidebar_accent = k.nav_hover;
        col.sidebar_accent_foreground = k.nav_ink;
        col.sidebar_border = k.nav_border;
        col.sidebar_primary = k.accent;
        col.sidebar_primary_foreground = k.on_accent;
        col.title_bar = k.nav_rail;
        col.title_bar_border = k.nav_border;
        col.tab_bar = k.surface_100;
        col.tab = k.surface_100;
        col.tab_active = k.surface_200;
        col.tab_active_foreground = k.ink;
        col.tab_foreground = k.ink_muted;
        col.tab_bar_segmented = k.surface_200;
        col.switch = k.border_strong;
        col.skeleton = k.surface_200;
        col.scrollbar = gpui_kit::transparent_black();
        col.scrollbar_thumb = k.border_strong;
        col.scrollbar_thumb_hover = k.ink_subtle;
        col.overlay = k.scrim;
        col.window_border = k.nav_border;
        col.drag_border = k.accent;
        col.drop_target = hsla(p.accent.with_alpha(0.12));
    });
}
