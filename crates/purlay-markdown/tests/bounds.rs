//! Pathological 16000-character bodies: each reads (or is refused) quickly,
//! without overflowing the stack.

use std::time::{Duration, Instant};

use purlay_markdown::{Reading, TypedReason, mentions, plain_text, read_markdown};

const BODY_MAX: usize = 16_000;

fn fill(unit: &str) -> String {
    unit.repeat(BODY_MAX / unit.len())
}

fn deep_containers(markers: &str) -> String {
    // Each line opens 32 containers, indented to continue the previous line's.
    let mut out = String::new();
    let mut indent = 0usize;
    while out.len() < BODY_MAX {
        out.push_str(&" ".repeat(indent));
        out.push_str(&markers.repeat(32));
        out.push_str("a\n");
        indent += markers.len() * 32;
    }
    out
}

fn bodies() -> Vec<(&'static str, String)> {
    vec![
        (
            "emphasis openers",
            format!("{}{}", "*".repeat(1000), fill("a")),
        ),
        (
            "alternating emphasis",
            format!("{}{}", "*a_".repeat(333), fill(" b")),
        ),
        ("brackets", format!("{}{}", "[".repeat(1000), fill("a "))),
        (
            "links",
            fill("[a](https://e.co) ").chars().take(BODY_MAX).collect(),
        ),
        ("backticks", format!("{}{}", "`a ".repeat(1000), fill(" b"))),
        ("tildes", format!("{}{}", "~".repeat(1000), fill("b "))),
        ("delimiters past the bound", fill("*a")),
        ("autolink-ish words", fill("www.a.b ")),
        ("long run", fill("a")),
        ("emails", fill("a@b.co ")),
        ("quotes per line", fill(&format!("{}a\n", "> ".repeat(32)))),
        ("lists per line", fill(&format!("{}a\n", "1. ".repeat(32)))),
        ("nested bullet lines", deep_containers("- ")),
        ("nested quote lines", deep_containers("> ")),
        ("indent staircase", {
            let mut s = String::new();
            let mut i = 0;
            while s.len() < BODY_MAX {
                s.push_str(&" ".repeat(i * 2));
                s.push_str("- a\n");
                i += 1;
            }
            s
        }),
        ("open fence", format!("```\n{}", fill("*_[`~|!<"))),
        (
            "table",
            format!(
                "|{}\n|{}\n{}",
                "a|".repeat(200),
                "-|".repeat(200),
                fill("|b|c\n")
            ),
        ),
        ("pipes", fill("|")),
        ("nul and cr", fill("a\0\r")),
    ]
}

#[test]
fn pathological_bodies_are_bounded() {
    let mut slow = Vec::new();
    for (name, body) in bodies() {
        let started = Instant::now();
        let reading = read_markdown(&body);
        if let Reading::Tree(tree) = &reading {
            let _ = plain_text(tree);
        }
        let _ = mentions(&body);
        let took = started.elapsed();
        println!(
            "{name}: {took:?} -> {:?}",
            reading.tree().map(|_| "tree").ok_or(&reading)
        );
        if took > Duration::from_secs(3) {
            slow.push(format!("{name}: {took:?}"));
        }
    }
    assert!(slow.is_empty(), "slow bodies: {slow:?}");
}

#[test]
fn reasons() {
    assert_eq!(
        read_markdown(&"*a".repeat(1001)),
        Reading::Typed(TypedReason::Delimiters)
    );
    assert_eq!(
        read_markdown(&"a".repeat(1025)),
        Reading::Typed(TypedReason::Run)
    );
    assert_eq!(
        read_markdown(&format!("{}x", "> ".repeat(33))),
        Reading::Typed(TypedReason::Nesting)
    );
    assert_eq!(
        read_markdown(&deep_containers("- ")),
        Reading::Typed(TypedReason::Depth)
    );
}
