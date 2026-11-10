//! Purlay's visual tokens, generated from the server bundle's `tokens.json`.
//!
//! Only `purlay-app`'s theme module turns these into GPUI colors and styles.

mod generated;

pub use generated::tokens::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Rgba { r, g, b, a }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Rgba { a, ..self }
    }

    /// WCAG relative luminance.
    pub fn luminance(self) -> f32 {
        fn ch(c: f32) -> f32 {
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        0.2126 * ch(self.r) + 0.7152 * ch(self.g) + 0.0722 * ch(self.b)
    }

    /// WCAG contrast ratio between two opaque colors.
    pub fn contrast(self, other: Rgba) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// `self` drawn over `bg`.
    pub fn over(self, bg: Rgba) -> Rgba {
        let a = self.a;
        Rgba::new(self.r * a + bg.r * (1.0 - a), self.g * a + bg.g * (1.0 - a), self.b * a + bg.b * (1.0 - a), 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub alpha: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeStyle {
    pub size: f32,
    /// A multiple of `size`.
    pub line_height: f32,
    pub weight: f32,
    pub tracking_em: f32,
    pub mono: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn palette(self) -> &'static Palette {
        match self {
            Theme::Light => &LIGHT,
            Theme::Dark => &DARK,
        }
    }
}

/// The avatar background for a username: chosen deterministically, the same on every client.
pub fn avatar_color(p: &Palette, key: &str) -> Rgba {
    let colors = [p.avatar_1, p.avatar_2, p.avatar_3, p.avatar_4, p.avatar_5, p.avatar_6];
    let mut h: u32 = 0;
    for b in key.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    colors[(h as usize) % colors.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_pairs_hold() {
        let mut failures = Vec::new();
        for (theme, p) in [("light", &LIGHT), ("dark", &DARK)] {
            for (fg, bg, min) in PAIRS {
                let (Some(f), Some(b)) = (color_by_name(p, fg), color_by_name(p, bg)) else { continue };
                let b = b.over(p.surface_100);
                let ratio = f.over(b).contrast(b);
                if ratio + 0.01 < *min {
                    failures.push(format!("{theme}: {fg} on {bg} = {ratio:.2} < {min}"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
