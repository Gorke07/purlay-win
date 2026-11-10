# Contributing to Purlay for Windows

Thank you for helping. This repository is the native Windows client; the server, the web app and the wire contract
live in [doancan/purlay](https://github.com/doancan/purlay), and the Mac app in
[doancan/purlay-macos](https://github.com/doancan/purlay-macos).

## Where does my change belong?

| You want to change… | Open it in |
| --- | --- |
| How the Windows app looks or behaves, Windows integration (tray, notifications, taskbar, shortcuts) | this repository |
| What the server stores or sends, an endpoint, a frame, a permission | [doancan/purlay](https://github.com/doancan/purlay/issues) |
| Behaviour that should be the same on every client | the server repository first, then here |

The client never invents wire types. If a feature needs something the contract does not have, open an issue on the
server (see [#1379](https://github.com/doancan/purlay/issues/1379) for an example) and, if a stopgap makes sense, keep
it on the device and say so in the code.

## Set up

You need Windows 10 1809 or later, Rust stable with the MSVC toolchain, the Windows SDK (Visual Studio Build Tools)
and the [GitHub CLI](https://cli.github.com) (only to fetch the contract).

```powershell
cargo xtask contract fetch    # the bundle pinned in Contract/contract.lock
cargo xtask generate          # wire types, routes, constants, design tokens
cargo run -p purlay-devserver -- --port 4390 --live --auto-sign-in ada
cargo run -p purlay-app -- --profile dev    # connect to http://127.0.0.1:4390
```

`--profile dev` keeps a separate set of settings and runs as its own instance, so it does not touch a real session.
`--auto-sign-in ada` makes the development server sign you straight in; without it the browser page lets you pick
one of the demo people (ada, grace, linus, mira, otto).

## Before you open a pull request

```powershell
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                      # including the end-to-end tests against the dev server
cargo xtask generate --check                # generated code matches the pinned contract
```

- **Style:** follow the code around you (long lines, one statement per builder step). The tree is not run through
  `rustfmt`, so do not reformat files you are not otherwise changing.
- **Generated code** (`crates/purlay-contract/src/generated`, `crates/purlay-tokens/src/generated`,
  `crates/purlay-core/src/generated`) is never edited by hand. Change the pin and run `cargo xtask generate`.
- **Strings** go in `crates/purlay-app/src/i18n.rs` with both English and Turkish. Turkish addresses the person as
  "sen", like the web's catalogue. A test checks that every key has both.
- **The UI reads the store with `read_recursive()`**, never `read()`: a nested `read()` can deadlock against the
  engine's writes.
- **Windows-only code** sits behind `#[cfg(windows)]` with a no-op beside it, so the crates still build elsewhere.
- **Tests:** behaviour of the core (engine, session, outbox) gets a test, preferably end to end against the dev
  server (`crates/purlay-core/tests/e2e.rs`). UI changes are checked by running the app; describe what you checked in
  the pull request.

## Commits

One topic per commit, an imperative subject of at most about 60 characters ("Mute a channel for a while"), and a
body that says why and how when it is not obvious. No generated noise in unrelated commits.

## Reporting bugs

Use the bug template. The app log is at `%LOCALAPPDATA%\Purlay\Purlay\data\logs\purlay.log`; run with
`PURLAY_LOG=info,purlay=debug` for more detail. Remove anything private (messages, addresses) before attaching it.

## License

By contributing you agree that your contribution is licensed under the [AGPL-3.0](LICENSE), like the rest of the
project.
