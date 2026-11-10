# Purlay for Windows

[![CI](https://github.com/Gorke07/purlay-win/actions/workflows/ci.yml/badge.svg)](https://github.com/Gorke07/purlay-win/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
![Windows 10 and 11](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4)

Your team's conversations, at home on Windows.

A native client for [Purlay](https://github.com/doancan/purlay), the self-hosted team chat, written in Rust with
[GPUI Kit](https://github.com/longbridge/gpui-kit). It is the Windows sibling of
[Purlay for macOS](https://github.com/doancan/purlay-macos): the same look, the same features, and the same pinned
wire contract, so it never drifts from what the server speaks.

> **Status:** in development, no release yet. Build it from source (below); every CI run on `main` also uploads a
> `purlay.exe` as the `purlay-windows-x64` artifact.

**Needs:** Windows 10 1809 or later (Windows 11 recommended), x64, and a Purlay server 0.5.0 or later. The app
connects to your server; it does not host one.

## Features

### Conversations

- **Channels, direct messages and group conversations**, with the server's sidebar groups and stars.
- **Threads** beside the feed: follow, resolve and reopen, questions and accepted answers.
- **Markdown** exactly as the server reads it: code blocks, lists, tables, links, `@mentions`; link previews.
- **Reactions** from a searchable emoji picker (English and Turkish names), with recents.
- **Files**: pick, paste or drop to upload; images inline, other files saved where you choose.
- **Search** across messages with question, code and file filters; **Saved** messages, per-channel **drafts**,
  **Open Topics**, and **Issues**, **scheduled messages** and **reminders** when the server offers them.
- **Sends that never get lost**: a message is painted at once, kept in an outbox until the server has it, and sent
  with the same id after a dropped connection or a restart.

### Notifications

- **Purlay's own banners** in the corner of your choice, stacked up to three, never taking the keyboard: who wrote,
  where, and two lines of the message. Hover to **reply** right there or **mark as read**; click to open the message.
- **Four sounds** made for Purlay (Drop, Chime, Marimba, Glass) or the Windows default, each with a preview.
- **Quiet when you need it**: nothing pops up during Windows' Do Not Disturb, full-screen apps and
  presentations; everything waits in the **Inbox** (Mentions, Direct, Threads, Updates).
- **Compact banners**, and banners **without the message text** for screen sharing.
- Prefer Windows toasts? One switch in Settings.

### At home on Windows

- **Runs in the notification area** when you close the window. The tray menu opens Purlay, sets **Online**, **Away**
  or **Deep Work**, and quits.
- **A count on the taskbar button**: a red badge with how many direct messages and mentions wait, kept current while
  the window is hidden.
- **A conversation in its own window** (`Ctrl O`), with a pin that keeps it above other windows.
- **Its own title bar** with Windows' snap layouts, light and dark themes that follow Windows, English and Turkish.
- **Launch at sign-in**, quietly in the notification area.

### Focus

- **Deep Work** and **Away** from the account menu, the tray or `Ctrl Shift D`.
- **Mute** a channel or conversation for 15 minutes, 1, 8 or 24 hours or until tomorrow morning; it comes back by
  itself.
- **Keyboard first**: walk channels and unread ones with the arrows, jump to the first unread message, mark read with
  `Esc` (see [Shortcuts](#shortcuts)).

## Get started

1. Build and run Purlay (see [Build](#build)).
2. Enter your server's address (for example `chat.example.com`) and choose **Continue**.
3. Choose **Open browser to sign in**. Your browser signs you in with whatever your server offers (GitHub, Google,
   email, password, single sign-on); approve Purlay there and the app continues on its own.

The first start registers the `purlay:` link type for your Windows account, which is how the browser hands the
sign-in back to the app, and how message links open in it.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Jump to a channel or person | `Ctrl K` |
| Search messages | `Ctrl Shift F` |
| Open Inbox | `Ctrl Shift I` |
| New conversation | `Ctrl N` |
| Next / previous channel | `Alt ↓` / `Alt ↑` |
| Next / previous unread channel | `Alt Shift ↓` / `Alt Shift ↑` |
| Jump to the first unread message | `Ctrl J` or `Shift Page Up` |
| Mark the channel read (when no panel is open) | `Esc` |
| Mark everything read | `Shift Esc` |
| Open the conversation in a new window | `Ctrl O` |
| Toggle Deep Work | `Ctrl Shift D` |
| Focus the message box | `Ctrl L` |
| Edit your last message | `↑` in an empty message box |
| Send (always) | `Ctrl Enter` |
| Settings | `Ctrl ,` |
| Show all shortcuts | `Ctrl /` or `Ctrl .` |

## Build

You need Rust (stable, MSVC toolchain), the Windows SDK (Visual Studio Build Tools) and the
[GitHub CLI](https://cli.github.com) to fetch the contract.

```powershell
cargo xtask contract fetch      # the server's contract bundle pinned in Contract/contract.lock
cargo xtask generate            # wire types, routes, constants and design tokens
cargo run -p purlay-app         # debug build
```

### Without a real server

`purlay-devserver` is an in-memory Purlay server with a seeded workspace (Acme) and demo people (ada, grace, linus,
mira, and otto the bot):

```powershell
cargo run -p purlay-devserver -- --port 4390 --live --auto-sign-in ada
cargo run -p purlay-app -- --profile dev   # then connect to http://127.0.0.1:4390
```

| Option | Does |
| --- | --- |
| `--live` / `--live-every SECONDS` | a bot posts, replies, reacts and mentions you now and then |
| `--auto-sign-in USER` | the sign-in page signs straight in as `USER` instead of offering a choice |
| `--empty` | people and channels, no messages |

`--profile NAME` (or `PURLAY_PROFILE`) keeps separate settings, drafts and outboxes and runs as its own instance, so a
development copy can sit beside your real session. While it runs, `purlay:` links open in that profile; the normal
app takes them back the next time it starts. Debug builds accept plain `http` for loopback addresses only.

## How it is built

```
Contract/contract.lock      the server contract this build speaks (server 0.6.1, contract 1)
xtask/                      fetch / verify the bundle; generate code from it
crates/purlay-contract      GENERATED serde types (379 schemas), the route table, constants
crates/purlay-tokens        GENERATED design tokens: palette per theme, spacing, type, sizes
crates/purlay-markdown      the server's markdown reader, ported; passes the bundle's conformance corpus
crates/purlay-core          no UI: API client, native sign-in, the realtime connection and engine, store, outbox
crates/purlay-platform      Windows: purlay: links, single instance, toasts, sounds, taskbar badge, Do Not Disturb
crates/purlay-app           the GPUI Kit interface, banners, tray and conversation windows
crates/purlay-devserver     an in-memory Purlay server for development and end-to-end tests
```

- **The contract is never written by hand.** `Contract/contract.lock` pins the server's bundle by the SHA-256 of its
  manifest; `cargo xtask contract verify` checks every file, and `cargo xtask generate --check` fails CI when the
  generated code is stale.
- **The realtime engine is a port of the web client's.** `realtime/connection.rs` is a sans-IO state machine with
  the same behaviour as `apps/web/src/realtime/connection.ts`, and its tests are a case-for-case port of
  `connection.test.ts` (91 cases). A single tokio task owns the socket, the timers and the store's writes.
- **Sign-in** is the server's native flow: PKCE challenge, the system browser, `purlay://auth`, then
  `POST /api/native/token` for the session cookie, kept in Windows Credential Manager.
- **Sends are optimistic and durable**: a UUIDv7 made against the server-corrected clock, painted at once, kept in
  `%LOCALAPPDATA%\Purlay\Purlay\data\outbox` until the server has it, retried with the same id.
- **Windows itself does the window chrome**: caption buttons and drag areas are hit-tested by Windows (so snap
  layouts work), the taskbar badge is an `ITaskbarList3` overlay, and Do Not Disturb is read from the shell's
  quiet-hours state. Notification sounds are synthesised at start, so the app ships no audio files.

## Tests

```powershell
cargo test --workspace
```

covers the generated types (tagged unions, unknown values, double options), the message-id vectors and markdown
corpus from the bundle, the connection state machine, handshake, sign-in, outbox and preferences, design token
contrast pairs, the notification sounds and taskbar badge, and the client end to end against the development server
(sending, live messages, reactions, threads, read state, reconnecting, the outbox after a restart, timed mutes).

## Known limitations

- **Not released yet**, so there is no installer or automatic update.
- **No notifications while the app is not running**: the server offers only Web Push. Keeping Purlay in the
  notification area (the default) covers this.
- **Timed mutes end on this device's clock**: the server cannot hold an end time yet
  ([doancan/purlay#1379](https://github.com/doancan/purlay/issues/1379)), so if Purlay is closed when a mute ends,
  it ends the next time Purlay starts.
- **No status message from the app**: the native contract cannot set one
  ([doancan/purlay#1380](https://github.com/doancan/purlay/issues/1380)); **urgent messages** wait on
  [doancan/purlay#1381](https://github.com/doancan/purlay/issues/1381).
- Do Not Disturb detection is tested on Windows 11. Windows 10's Focus assist publishes the same signal but has not
  been tested.
- The server labels native sessions "Purlay for macOS" and has no `minClient.windows` entry yet, so it cannot ask a
  Windows client to update.
- Workspace administration, invitations, your profile and server settings open in the browser, as on the Mac.

## Contributing

Issues and pull requests are welcome; read [CONTRIBUTING.md](CONTRIBUTING.md) first. Server-side ideas belong in
[doancan/purlay](https://github.com/doancan/purlay). Report security problems privately as described in
[SECURITY.md](SECURITY.md). This project follows a [code of conduct](CODE_OF_CONDUCT.md).

## License

[AGPL-3.0](LICENSE). See [NOTICE](NOTICE) for third-party components.
