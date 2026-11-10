//! The conformance corpus: every case's reading must equal the server's.

use std::path::PathBuf;

use purlay_markdown::{
    MARKDOWN_DELIMITERS_MAX, MARKDOWN_DEPTH_MAX, MARKDOWN_NESTING_MAX, MARKDOWN_RUN_MAX,
    read_markdown,
};
use serde_json::Value;

/// Cases this reader cannot match, each with why. Keep it empty if at all possible.
const KNOWN_DIVERGENCES: &[(&str, &str)] = &[];

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../Contract/bundle/purlay-contract-v0.6.1/fixtures/markdown-corpus.json")
}

#[test]
fn corpus() {
    let path = corpus_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        println!("markdown corpus not found at {}; skipping", path.display());
        return;
    };
    let mut de = serde_json::Deserializer::from_str(&raw);
    de.disable_recursion_limit();
    let corpus: Value = serde::Deserialize::deserialize(&mut de).expect("corpus is JSON");

    let limits = &corpus["limits"];
    assert_eq!(limits["depthMax"], MARKDOWN_DEPTH_MAX as u64);
    assert_eq!(limits["delimitersMax"], MARKDOWN_DELIMITERS_MAX as u64);
    assert_eq!(limits["nestingMax"], MARKDOWN_NESTING_MAX as u64);
    assert_eq!(limits["runMax"], MARKDOWN_RUN_MAX as u64);

    let cases = corpus["cases"].as_array().expect("cases");
    let mut failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut passed = 0usize;
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let input = case["input"].as_str().expect("input");
        let actual = read_markdown(input).to_json();
        let ok = actual == case["expected"];
        let known = KNOWN_DIVERGENCES.iter().any(|(n, _)| *n == name);
        match (ok, known) {
            (true, false) => passed += 1,
            (true, true) => unexpected_passes.push(name.to_owned()),
            (false, true) => {}
            (false, false) => failures.push(format!(
                "{name}\n  input:    {input:?}\n  expected: {}\n  actual:   {}",
                truncate(&case["expected"].to_string()),
                truncate(&actual.to_string()),
            )),
        }
    }
    println!("markdown corpus: {passed}/{} cases pass", cases.len());
    assert!(
        failures.is_empty(),
        "{} of {} cases fail:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
    assert!(
        unexpected_passes.is_empty(),
        "known divergences that now pass (remove them): {unexpected_passes:?}"
    );
}

fn truncate(s: &str) -> String {
    if s.len() > 600 {
        let mut end = 600;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    } else {
        s.to_owned()
    }
}
