//! Identifier helpers shared by the generators.

pub fn pascal(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            if upper {
                out.push(ch.to_ascii_uppercase());
                upper = false;
            } else if ch.is_ascii_uppercase() && !prev_lower && is_all_caps_word(s) {
                out.push(ch.to_ascii_lowercase());
            } else {
                out.push(ch);
            }
            prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            upper = true;
            prev_lower = false;
        }
    }
    if out.is_empty() {
        out.push_str("Empty");
    }
    if out.chars().next().unwrap().is_ascii_digit() {
        out.insert(0, 'V');
    }
    out
}

fn is_all_caps_word(s: &str) -> bool {
    s.chars().all(|c| !c.is_ascii_lowercase())
}

pub fn snake(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase() {
            let prev = if i > 0 { chars[i - 1] } else { '_' };
            let next_lower = chars.get(i + 1).is_some_and(|c| c.is_ascii_lowercase());
            if i > 0 && prev != '_' && (prev.is_ascii_lowercase() || prev.is_ascii_digit() || next_lower) {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "value".into() } else { out }
}

pub fn shouty(s: &str) -> String {
    snake(s).to_ascii_uppercase()
}

const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for", "if", "impl",
    "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "static", "struct",
    "super", "trait", "true", "type", "unsafe", "use", "where", "while", "async", "await", "dyn", "abstract",
    "become", "box", "do", "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
    "gen",
];

pub fn field_ident(json_name: &str) -> String {
    let s = snake(json_name);
    let s = if s.chars().next().is_some_and(|c| c.is_ascii_digit()) { format!("n{s}") } else { s };
    if KEYWORDS.contains(&s.as_str()) { format!("r#{s}") } else { s }
}

pub fn doc(text: &str, indent: &str) -> String {
    text.lines().map(|l| format!("{indent}/// {}\n", l.trim_end())).collect()
}

pub fn rust_str(s: &str) -> String {
    format!("{s:?}")
}
