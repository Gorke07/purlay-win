//! What bounds the parser's cost (server `scan.ts`).

/// Delimiters outside code beyond which a body is shown as typed.
pub const MARKDOWN_DELIMITERS_MAX: usize = 1000;
/// Containers one line may open beyond which a body is shown as typed.
pub const MARKDOWN_NESTING_MAX: usize = 32;
/// The longest address-like run outside code beyond which a body is shown as typed.
pub const MARKDOWN_RUN_MAX: usize = 1024;

/// A half-open range of byte offsets `[start, end)` the engine read as code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeRange {
    pub start: usize,
    pub end: usize,
}

/// What the inline reading will cost, measured outside code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProseMeasure {
    pub delimiters: usize,
    pub longest_run: usize,
}

/// Lines split on `\r\n`, `\r` or `\n`.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut at = 0usize;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        while at < bytes.len() {
            match bytes[at] {
                b'\n' => {
                    let line = &text[start..at];
                    at += 1;
                    start = at;
                    return Some(line);
                }
                b'\r' => {
                    let line = &text[start..at];
                    at += 1;
                    if at < bytes.len() && bytes[at] == b'\n' {
                        at += 1;
                    }
                    start = at;
                    return Some(line);
                }
                _ => at += 1,
            }
        }
        done = true;
        Some(&text[start..])
    })
}

/// Containers opened at the start of one line:
/// `^[ \t]*(?:>|(?:[*+-]|\d{1,9}[.)])(?=[ \t]|$))`, repeated.
fn nesting_of(line: &str) -> usize {
    let b = line.as_bytes();
    let mut at = 0usize;
    let mut nesting = 0usize;
    loop {
        let mut i = at;
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        if i >= b.len() {
            return nesting;
        }
        let end = if b[i] == b'>' {
            Some(i + 1)
        } else {
            let marker_end = if matches!(b[i], b'*' | b'+' | b'-') {
                Some(i + 1)
            } else {
                let mut j = i;
                while j < b.len() && j - i < 9 && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i && j < b.len() && matches!(b[j], b'.' | b')') {
                    Some(j + 1)
                } else {
                    None
                }
            };
            marker_end.filter(|&e| e == b.len() || b[e] == b' ' || b[e] == b'\t')
        };
        match end {
            Some(e) => {
                nesting += 1;
                if nesting > MARKDOWN_NESTING_MAX {
                    return nesting;
                }
                at = e;
            }
            None => return nesting,
        }
    }
}

/// Whether some line opens more containers than the engine is allowed to nest.
pub fn nests_too_deep(text: &str) -> bool {
    lines(text).any(|line| nesting_of(line) > MARKDOWN_NESTING_MAX)
}

const fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'*' | b'_' | b'[' | b']' | b'`' | b'~' | b'|' | b'!' | b'<'
    )
}

const fn is_run(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+')
}

/// Measures the text outside the given code ranges (in order, not overlapping).
///
/// Works on bytes: every delimiter and run character is ASCII, and any
/// non-ASCII character breaks a run, so this agrees with the server's UTF-16
/// walk.
pub fn measure_outside(text: &str, code: &[CodeRange]) -> ProseMeasure {
    let b = text.as_bytes();
    let mut m = ProseMeasure::default();
    let mut run = 0usize;
    let mut next = 0usize;
    let mut at = 0usize;
    while at < b.len() {
        if let Some(range) = code.get(next)
            && at >= range.start
        {
            at = range.end.max(at + 1);
            next += 1;
            run = 0;
            continue;
        }
        let c = b[at];
        if is_delimiter(c) {
            m.delimiters += 1;
        }
        run = if is_run(c) { run + 1 } else { 0 };
        m.longest_run = m.longest_run.max(run);
        at += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting() {
        assert_eq!(nesting_of("> > - 1. x"), 4);
        assert_eq!(nesting_of("-a"), 0);
        assert_eq!(nesting_of("1234567890. x"), 0);
        assert_eq!(nesting_of("-"), 1);
        assert!(nests_too_deep(&">".repeat(33)));
        assert!(!nests_too_deep(&">".repeat(32)));
        assert!(nests_too_deep(&format!("a\r{}", "> ".repeat(33))));
    }
}
