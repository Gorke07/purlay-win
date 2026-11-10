//! The addresses a link may carry (server `href.ts`).

/// The address as the WHATWG URL parser serialises it (`new URL(url).href`),
/// if it parses on its own and its scheme is http, https or mailto; `None`
/// otherwise (relative, `//host`, `javascript:` …).
pub fn safe_href(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    match parsed.scheme() {
        "http" | "https" | "mailto" => Some(parsed.into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::safe_href;

    #[test]
    fn normalises_and_refuses() {
        assert_eq!(
            safe_href("HTTPS://EXAMPLE.com:443").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(safe_href("javascript:alert(1)"), None);
        assert_eq!(safe_href("/path"), None);
        assert_eq!(safe_href("//evil.test"), None);
        assert_eq!(safe_href("ftp://example.com"), None);
        assert_eq!(
            safe_href("mailto:a@example.com").as_deref(),
            Some("mailto:a@example.com")
        );
    }
}
