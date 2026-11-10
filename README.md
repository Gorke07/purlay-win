# Purlay for Windows

Your team's conversations, at home on Windows.

A native client for [Purlay](https://github.com/doancan/purlay), the self-hosted team chat, written in Rust with
[GPUI Kit](https://github.com/longbridge/gpui-kit). It is the Windows sibling of
[Purlay for macOS](https://github.com/doancan/purlay-macos) and speaks the same pinned contract.

**Requirements:** Windows 10 1809 or later (Windows 11 recommended), x64, and a Purlay server 0.5.0 or later. The app
connects to your server; it does not host one.

## Features

| Stay connected | Stay focused |
| --- | --- |
| **Channels, direct messages and group conversations**, with server-kept sidebar groups and stars. | **Inbox**: Mentions, Direct, Threads and Updates, with an Unread filter. |
| **Threads** beside the feed: follow, resolve and reopen, questions and accepted answers. | **Deep Work** and **Away**, from the workspace menu or Ctrl+Shift+D. |
| **Reactions** from a searchable emoji picker (English and Turkish), with recents. | **Saved messages** (In Progress / Completed / Archived), **drafts** per channel and thread. |
| **Files**: pick, paste or drop to upload; images inline, files saved where you choose. | **Open Topics**: unresolved threads across the workspace. |
| **Markdown** exactly as the server reads it: code blocks, lists, tables, links, `@mentions`. | **Search** across messages, with question, code and file filters, and other modules' hits. |
| **Issues**, **scheduled messages** and **reminders** when the server offers them. | **Notifications** in Purlay's own banners (or Windows toasts): reply or mark read from the banner, four sounds to choose from, and quiet during Windows' Do Not Disturb, full-screen apps and presentations. |
| **Mute** a channel or conversation for 15 minutes up to a day; it comes back by itself. | **Notification area**: Purlay keeps running when closed; set Online, Away or Deep Work from the tray; the taskbar button shows how many messages wait. |

Also: optimistic sends with a durable outbox (a message written while offline is sent when the connection is back,
and survives a restart), live presence and typing, link previews, edit history, permalinks
(`purlay://open?url=…`), English and Turkish, light and dark themes following Windows, comfortable or compact
messages, and launch at sign-in.

## Get started

1. Download the latest release and run `purlay.exe`.
2. Enter your server's address (for example `chat.example.com`) and choose **Continue**.
3. Choose **Open browser to sign in**. Your browser signs you in with whatever your server offers (GitHub, Google,
   email, password, single sign-on); approve Purlay there and the app continues on its own.

The first start registers the `purlay:` link type for your Windows account, which is how the browser hands the
sign-in back to the app.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Jump to a channel or person | `Ctrl K` |
| Search messages | `Ctrl Shift F` |
| Open Inbox | `Ctrl Shift I` |
| Toggle Deep Work | `Ctrl Shift D` |
| New conversation | `Ctrl N` |
| Next / previous channel | `Alt ↓` / `Alt ↑` |
| Next / previous unread channel | `Alt Shift ↓` / `Alt Shift ↑` |
| Jump to the first unread message | `Ctrl J` or `Shift Page Up` |
| Mark the channel read (when no panel is open) | `Esc` |
| Mark everything read | `Shift Esc` |
| Focus the message box | `Ctrl L` |
| Edit your last message | `↑` in an empty message box |
| Send (always) | `Ctrl Enter` |
| Show all shortcuts | `Ctrl /` or `Ctrl .` |

## Build

You need Rust (stable, MSVC toolchain), the Windows SDK (Visual Studio Build Tools) and the
[GitHub CLI](https://cli.github.com) to fetch the contract.

```powershell
cargo xtask contract fetch      # the server's contract bundle pinned in Contract/contract.lock
cargo xtask generate            # wire types, routes, constants and design tokens
cargo run -p purlay-app         # debug build
cargo build --release -p purlay-app
```

To try it without a real server, run the in-memory development server and sign in as one of its demo people:

```powershell
cargo run -p purlay-devserver -- --port 4390 --live
cargo run -p purlay-app -- --profile dev   # then connect to http://127.0.0.1:4390
```

`--profile NAME` (or `PURLAY_PROFILE`) keeps separate settings, drafts and outboxes and runs as its own instance, so
a development copy can sit beside your real session. While it runs, `purlay:` links open in that profile; the normal
app takes them back the next time it starts. Debug builds accept plain `http` for loopback addresses only.

## How it is built

```
Contract/contract.lock      the server contract this build speaks (server 0.6.1, contract 1)
xtask/                      fetch / verify the bundle; generate code from it
crates/purlay-contract      GENERATED serde types (379 schemas), the route table, constants
crates/purlay-tokens        GENERATED design tokens: palette per theme, spacing, type, sizes
crates/purlay-markdown      the server's markdown reader, ported; passes the bundle's conformance corpus
crates/purlay-core          no UI: API client, native sign-in, the realtime connection and engine, store, outbox
crates/purlay-platform      Windows: purlay: links, single instance, toasts, launch at sign-in
crates/purlay-app           the GPUI Kit interface
crates/purlay-devserver     an in-memory Purlay server for development and end-to-end tests
```

- **The contract is never written by hand.** `Contract/contract.lock` pins the server's bundle by the SHA-256 of its
  manifest; `cargo xtask contract verify` checks every file, and `cargo xtask generate --check` fails CI when the
  generated code is stale.
- **The realtime engine is a port of the web client's.** `realtime/connection.rs` is a sans-IO state machine with
  the same behaviour as `apps/web/src/realtime/connection.ts`, and its tests are a case-for-case port of
  `connection.test.ts` (91 cases). The tokio driver performs its socket and timer actions.
- **Sign-in** is the server's native flow (D-191): PKCE challenge, the system browser, `purlay://auth`, then
  `POST /api/native/token` for the session cookie, kept in Windows Credential Manager.
- **Sends are optimistic and durable**: a UUIDv7 made against the server-corrected clock, painted at once, kept in
  `%LOCALAPPDATA%\Purlay\data\outbox` until the server has it, retried with the same id.

## Tests

```powershell
cargo test --workspace
```

covers the generated types (tagged unions, unknown values, double options), the message-id vectors and markdown
corpus from the bundle, the connection state machine, handshake, sign-in, outbox and preferences, design token
contrast pairs, and the development server end to end.

## Known limitations

- The server labels native sessions "Purlay for macOS" and its consent page says "Open Purlay for Mac?"; it has no
  `minClient.windows` entry yet, so it cannot ask a Windows client to update.
- There is no push when the app is not running (the server only offers Web Push); while it runs, notifications are
  local toasts.
- Workspace administration, invitations and server settings open in the browser, as on the Mac.

## License

[AGPL-3.0](LICENSE). See [NOTICE](NOTICE) for third-party components.
