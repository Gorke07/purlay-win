## What and why

<!-- What changes for the person using Purlay, and why. Link the issue it closes. -->

## How it was checked

<!-- Tests added or run, and what you tried in the app (light and dark theme, Turkish, narrow window…). -->

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] `cargo xtask generate --check` passes; no generated file was edited by hand
- [ ] New strings have both English and Turkish in `i18n.rs`
- [ ] Nothing here needs a server change, or the server issue is linked
