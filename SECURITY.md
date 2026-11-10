# Security policy

## Reporting a vulnerability

Please do not open a public issue for a security problem. Use GitHub's private
[security advisory form](https://github.com/Gorke07/purlay-win/security/advisories/new) for this repository, and
include what you found, how to reproduce it, and what an attacker could do with it.

You will get an answer within a week. Fixes are released as soon as they are ready, and the advisory is published
with credit to you unless you ask otherwise.

If the problem is in the Purlay server, the web app or the wire contract rather than in this Windows client, report it
to [doancan/purlay](https://github.com/doancan/purlay/security) instead.

## Supported versions

Only the latest release receives security fixes.

## What the client keeps on the device

| What | Where |
| --- | --- |
| The session cookie | Windows Credential Manager: service `dev.purlay.windows.session`, one entry per server |
| Settings, drafts, recent searches, timed mutes | `%APPDATA%\Purlay\Purlay\config\prefs.json` |
| Messages not yet sent (the outbox) | `%LOCALAPPDATA%\Purlay\Purlay\data\outbox` |
| Diagnostic logs | `%LOCALAPPDATA%\Purlay\Purlay\data\logs` |

Signing out deletes the session from Credential Manager, asks the server to end it, and forgets the account's
settings on this device.
