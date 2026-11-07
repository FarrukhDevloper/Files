# VPS Files — Architecture

A Finder-style desktop file manager that browses the local disk and every
host in `~/.ssh/config` side by side, copies between any two of them
(including VPS → VPS), and plays media straight from a server.

Targets: Linux + Windows first, macOS later. Hard requirement: **light** —
idle CPU ≈ 0 %, RAM well under 150 MB.

## Stack

| Layer | Choice |
|---|---|
| Shell | Tauri 2 (system webview: WebView2 / WebKitGTK / WKWebView) |
| Backend | Rust 2024, tokio |
| SSH / SFTP | `russh` (ring backend) + `russh-sftp` (pure Rust, no OpenSSL/libssh2) |
| SSH config | own parser (`crates/ssh/src/config.rs`) — OpenSSH semantics incl. Include/Match all |
| Media server | `axum` on `127.0.0.1:0` |
| Frontend | Svelte 5 (runes) + TypeScript + Vite, no SvelteKit |
| Icons / font | `@lucide/svelte`, Inter Variable (bundled, offline) |

## Layout

```
Cargo.toml            workspace
crates/
  vfs/                Backend trait, Entry/Location/SourceId, VfsError, path rules, LocalBackend
  ssh/                ~/.ssh/config, SshManager (connections, status, exec), SftpBackend
  transfer/           TransferManager: queue, copy/move any→any, progress, pause/cancel
  stream/             MediaServer: HTTP Range streaming of any VFS file + thumbnails
  edit/               open remote files in local apps, sync saves back
src-tauri/            Tauri shell: AppState, commands, event forwarding, UI prompter
src/                  Svelte UI
  lib/ipc/            types.ts (contract), api.ts (typed invoke), mock.ts (browser backend)
  lib/stores/         app state (runes)
  lib/components/     Finder UI
  styles/tokens.css   design tokens
docs/                 this file, IPC.md, DESIGN.md, TASKS.md
```

## Core idea: one trait for every filesystem

```
            ┌──────────── src-tauri (commands) ────────────┐
 Svelte ──► │ list_dir(Location) ─► Resolver.backend(src) ─┼─► LocalBackend   (std::fs)
  UI   ◄──  │ events: conn:status, transfer:update, prompt │─► SftpBackend    (russh-sftp)
            └───────────┬───────────────────┬──────────────┘
                        │                   │
                TransferManager        MediaServer
                (uses Resolver)        (uses Resolver)
```

* `vpsfiles_vfs::Backend` — `list / stat / mkdir / remove_* / rename /
  open_read(offset) / open_write(mode) / remove_all / trash`.
* `SourceId` is `Local` or `Ssh { host }` where `host` is the `~/.ssh/config`
  alias. A `Location` is `(SourceId, path)`.
* `BackendResolver` maps a `SourceId` to a live `Arc<dyn Backend>`, connecting
  SSH on demand. Transfer and stream code only ever see the trait, so
  local→VPS, VPS→local and VPS→VPS are the same code path.

### Path rules (`crates/vfs/src/path.rs`)

Absolute, `/`-separated, no trailing slash except roots. Remote and local
Unix are POSIX. Local Windows is `C:/Users/me`; `/` is a virtual root that
lists drives. The UI never builds paths by string concatenation of user
input — it uses names returned by the backend or sends names to commands
that validate them.

### Errors

Every failure is a `VfsError` that serializes as
`{ code, message }`. The UI branches on `code` (`auth`, `host_key`,
`not_connected`, `already_exists`…).

## SSH

* Hosts come from `~/.ssh/config` (`Include`, `Host *` defaults applied,
  wildcard-only blocks not listed). Re-read on every `list_hosts`.
* One connection + one SFTP session per alias, opened on first use,
  shared by browsing, transfers and streaming. Concurrent connects coalesce.
* Auth order: ssh-agent → `IdentityFile`s (passphrase via prompt) →
  password (via prompt).
* Host keys: checked against `~/.ssh/known_hosts`. Unknown → ask the user
  (TOFU) and append on accept. **Changed → hard fail** (`host_key`), never
  prompted, same as OpenSSH.
* Prompts go through the `Prompter` trait. The app implements it by
  emitting `prompt:request` and waiting for `prompt_respond`.
* Keepalive, and a dead connection flips status to `disconnected` so the
  next call reconnects.

## Transfers

* `TransferManager` runs ≤ 3 jobs in parallel, more stay `queued`.
* A job: scan sources (totals) → copy each file with a 256 KiB buffer,
  emitting snapshots at most ~5×/s → `completed | failed | cancelled`.
* Same-source move = `rename`; cross-source move = copy then delete.
* Conflict policy per request: `overwrite` (Finder Replace — whole item,
  new copy staged under a hidden `.name.vpsfiles-xxxx.part` sibling and
  swapped in last, so failures never destroy the original), `merge`
  (folder into folder), `skip`, `keepBoth`.
* VPS → VPS: **relay** first (stream through this machine, never touches
  disk). **Direct** mode (run `rsync`/`scp` on the source host so bytes go
  server-to-server) comes later via `SshManager::exec`.
* Resume: `WriteMode::Append` + `open_read(offset)` when the destination is
  a shorter prefix of the source.

## Media

`MediaServer` binds `127.0.0.1:0` lazily on the first `media_url` call. URL:
`http://127.0.0.1:{port}/{token}/{source}/{path…}` (each segment percent-encoded, e.g. `…/ssh%3Atanla/srv/a.mp4`). It answers `Range`
requests via `open_read(offset)` + length limit, so `<video>` can seek a
multi-GB remote file instantly. The webview plays what it supports
(mp4/H.264, webm, mp3, flac, ogg, images); others get an "open in mpv/VLC"
fallback with the same URL.

## Opening remote files in local apps (`crates/edit`)

`open_external` on a remote file downloads it to
`<cache>/vps-files/edit/<hash>/<original name>` (the name is kept so the OS
picks the right app), opens it with the default app and watches its folder
(`notify`). Each save is uploaded after a 700 ms debounce, **in place**
(truncate + write) so the server file keeps its permissions and owner. If
the server copy changed since the last sync, nothing is written and the
session goes to `conflict` until the user picks upload or discard. Media
can instead be streamed to an external player (mpv, VLC, …) found on PATH
or in the default Windows install folders.

## Lightness rules

* No polling. Remote dirs refresh on navigation/focus/explicit action;
  local watching (`notify`) comes later and is debounced.
* Listings run in one blocking task, not one future per entry.
* Long lists are virtualized in the UI (render only visible rows).
* No animation loops when idle; CSS transitions only on interaction.
* `backdrop-filter` sparingly — expensive on WebKitGTK.
* Release profile: LTO, `codegen-units = 1`, `strip`, `panic = "abort"`.

### Measured (2026-11-07, Linux, release build, Xvfb, home dir open)

| | |
|---|---|
| Binary (frontend embedded) | 12.2 MB |
| Idle CPU over 10 s | 0 ticks |
| PSS: app + WebKit web + network | 58 + 92 + 16 = **~163 MB** |
| Frontend JS | 145 KB (49 KB gzip) |

Most of the memory is WebKitGTK's baseline, not our code. Re-measure on
Windows (WebView2) and on a real GPU session before drawing conclusions.

## Security

* CSP limits network to IPC and `127.0.0.1` (media server).
* Media server: loopback only + random per-launch token in every URL.
* Passwords never stored in this phase; later via OS keychain (`keyring`).
* Delete of a root path is refused server-side.

## Roadmap

1. **MVP**: hosts, browse local + SFTP, mkdir/rename/delete, copy/move with
   queue, list + icon views, Quick Look for text/images/media.
2. **v1**: direct VPS→VPS, resume, column view, tabs, search, drag & drop,
   keychain, installers + auto-update, local watching.
3. **Polish + macOS**: ProxyJump, thumbnails, vibrancy, signing/notarization,
   drag-out to the OS.
