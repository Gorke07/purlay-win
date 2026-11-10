//! tokens.json / pairs.json → Rust constants (palette per theme, metrics, type styles).

use crate::naming::{doc, field_ident, rust_str, shouty};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fmt::Write;

fn parse_color(s: &str) -> Result<(f32, f32, f32, f32)> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let v = u32::from_str_radix(hex, 16)?;
        return Ok(match hex.len() {
            6 => (((v >> 16) & 0xff) as f32 / 255.0, ((v >> 8) & 0xff) as f32 / 255.0, (v & 0xff) as f32 / 255.0, 1.0),
            8 => (
                ((v >> 24) & 0xff) as f32 / 255.0,
                ((v >> 16) & 0xff) as f32 / 255.0,
                ((v >> 8) & 0xff) as f32 / 255.0,
                (v & 0xff) as f32 / 255.0,
            ),
            _ => bail!("color {s}"),
        });
    }
    if let Some(body) = s.strip_prefix("rgba(").and_then(|b| b.strip_suffix(')')) {
        let parts: Vec<f32> = body.split(',').map(|p| p.trim().parse::<f32>()).collect::<Result<_, _>>()?;
        return Ok((parts[0] / 255.0, parts[1] / 255.0, parts[2] / 255.0, parts[3]));
    }
    bail!("unsupported color {s}")
}

fn px(s: &str) -> Result<f32> {
    if s == "9999px" {
        return Ok(9999.0);
    }
    s.trim_end_matches("px").parse::<f32>().with_context(|| format!("px {s}"))
}

fn f(v: f32) -> String {
    let s = format!("{v}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

pub fn generate(tokens: &Value, pairs: &Value) -> Result<String> {
    let mut o = String::new();
    writeln!(o, "#![allow(clippy::all)]\n\nuse crate::Rgba;\n")?;
    writeln!(o, "pub const TOKENS_VERSION: i64 = {};\n", tokens["version"])?;

    // Colors
    let themes: Vec<String> = tokens["color"]["themes"]
        .as_array()
        .context("themes")?
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    let colors = tokens["color"]["tokens"].as_array().context("color tokens")?;
    writeln!(o, "/// Every color token for one theme.\n#[derive(Debug, Clone, Copy, PartialEq)]\npub struct Palette {{")?;
    for c in colors {
        let name = c["name"].as_str().unwrap();
        if let Some(u) = c["usage"].as_str() {
            o.push_str(&doc(u, "    "));
        }
        writeln!(o, "    pub {}: Rgba,", field_ident(name))?;
    }
    writeln!(o, "}}\n")?;
    for theme in &themes {
        writeln!(o, "pub const {}: Palette = Palette {{", shouty(theme))?;
        for c in colors {
            let name = c["name"].as_str().unwrap();
            let mut raw = c["value"][theme].as_str().context("color value")?.to_string();
            for _ in 0..4 {
                if let Some(r) = raw.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
                    let target = colors.iter().find(|x| x["name"] == r).context("color ref")?;
                    raw = target["value"][theme].as_str().unwrap().to_string();
                }
            }
            let (r, g, b, a) = parse_color(&raw)?;
            writeln!(o, "    {}: Rgba::new({}, {}, {}, {}),", field_ident(name), f(r), f(g), f(b), f(a))?;
        }
        writeln!(o, "}};\n")?;
    }

    // Plain px scales
    for (section, module, prefix) in [
        ("spacing", "space", "space-"),
        ("radius", "radius", "radius-"),
        ("size", "size", "size-"),
        ("borderWidth", "stroke", "stroke-"),
    ] {
        writeln!(o, "pub mod {module} {{")?;
        for t in tokens[section]["tokens"].as_array().context(section)? {
            let name = t["name"].as_str().unwrap();
            if let Some(u) = t["usage"].as_str() {
                o.push_str(&doc(u, "    "));
            }
            let short = name.trim_start_matches(prefix);
            let id = if short.starts_with(|c: char| c.is_ascii_digit()) { shouty(name) } else { shouty(short) };
            writeln!(o, "    pub const {id}: f32 = {};", f(px(t["value"].as_str().unwrap())?))?;
        }
        writeln!(o, "}}\n")?;
    }
    writeln!(o, "pub mod duration {{")?;
    for t in tokens["duration"]["tokens"].as_array().context("duration")? {
        let name = t["name"].as_str().unwrap();
        let ms: u64 = t["value"].as_str().unwrap().trim_end_matches("ms").parse()?;
        writeln!(o, "    pub const {}: u64 = {ms};", shouty(name.trim_start_matches("duration-")))?;
    }
    writeln!(o, "}}\n")?;
    writeln!(o, "pub mod z {{")?;
    for t in tokens["zIndex"]["tokens"].as_array().context("zIndex")? {
        let name = t["name"].as_str().unwrap();
        writeln!(o, "    pub const {}: u16 = {};", shouty(name.trim_start_matches("z-")), t["value"].as_str().unwrap())?;
    }
    writeln!(o, "}}\n")?;

    // Typography
    writeln!(o, "pub const FONT_SANS_CSS: &str = {};", rust_str(tokens["type"]["families"]["sans"].as_str().unwrap_or("")))?;
    writeln!(o, "pub const FONT_MONO_CSS: &str = {};\n", rust_str(tokens["type"]["families"]["mono"].as_str().unwrap_or("")))?;
    writeln!(o, "pub mod typography {{\n    use crate::TypeStyle;")?;
    for group in tokens["type"]["groups"].as_array().context("type groups")? {
        let mono = group["family"].as_str() == Some("mono");
        for s in group["styles"].as_array().context("styles")? {
            let name = s["name"].as_str().unwrap();
            let size = px(s["fontSize"].as_str().unwrap())?;
            let lh = s["lineHeight"].as_f64().unwrap_or(1.5) as f32;
            let weight = s["fontWeight"].as_f64().unwrap_or(400.0) as f32;
            let tracking = s["letterSpacing"].as_str().map(|t| t.trim_end_matches("em").parse::<f32>().unwrap_or(0.0)).unwrap_or(0.0);
            if let Some(u) = s["usage"].as_str() {
                o.push_str(&doc(u, "    "));
            }
            writeln!(o, "    pub const {}: TypeStyle = TypeStyle {{ size: {}, line_height: {}, weight: {}, tracking_em: {}, mono: {mono} }};", shouty(name), f(size), f(lh), f(weight), f(tracking))?;
        }
    }
    writeln!(o, "}}\n")?;

    // Shadows
    writeln!(o, "pub mod shadow {{\n    use crate::Shadow;")?;
    for t in tokens["shadow"]["tokens"].as_array().context("shadow")? {
        let name = shouty(t["name"].as_str().unwrap().trim_start_matches("shadow-"));
        for theme in &themes {
            let Some(v) = t["value"][theme].as_str() else { continue };
            // "0 4px 12px 0 rgba(0, 0, 0, 0.05)"
            let (geom, color) = v.split_once("rgba").context("shadow color")?;
            let nums: Vec<f32> = geom.split_whitespace().map(|n| px(n).unwrap_or(0.0)).collect();
            let (_, _, _, a) = parse_color(&format!("rgba{color}"))?;
            writeln!(o, "    pub const {name}_{}: Shadow = Shadow {{ x: {}, y: {}, blur: {}, spread: {}, alpha: {} }};", shouty(theme), f(nums[0]), f(nums[1]), f(nums[2]), f(*nums.get(3).unwrap_or(&0.0)), f(a))?;
        }
    }
    writeln!(o, "}}\n")?;

    // Contrast pairs (checked by a test)
    writeln!(o, "/// (foreground, background, minimum WCAG contrast ratio)\npub const PAIRS: &[(&str, &str, f32)] = &[")?;
    for p in pairs["pairs"].as_array().context("pairs")? {
        writeln!(o, "    ({}, {}, {}),", rust_str(p["foreground"].as_str().unwrap()), rust_str(p["background"].as_str().unwrap()), f(p["minimum"].as_f64().unwrap_or(4.5) as f32))?;
    }
    writeln!(o, "];\n")?;
    writeln!(o, "/// Looks a color token up by its bundle name (`ink-muted`).\npub fn color_by_name(p: &Palette, name: &str) -> Option<Rgba> {{\n    Some(match name {{")?;
    for c in colors {
        let name = c["name"].as_str().unwrap();
        writeln!(o, "        {} => p.{},", rust_str(name), field_ident(name))?;
    }
    writeln!(o, "        _ => return None,\n    }})\n}}")?;
    Ok(o)
}
