# Roadmap

Where the app is going after rounds 1–3. The product goal hasn't changed:
a Finder-style file manager that is **light** (idle CPU ≈ 0 %, ~135 MB),
fast, and treats every place your files live — this computer, your
servers, your clouds — as one disk.

## Where we are (2026-11-07)

Done and running on Linux (release build, tested against real servers):

* Local disk, drives, trash; every host from `~/.ssh/config` plus servers
  added in the app (keychain-stored secrets), ProxyJump, quiet auto-connect
  at startup, background auto-reconnect.
* SFTP at OpenSSH speed (64 × 32 KiB pipelining, ~6 MiB/s on a 130 ms link;
  directory listing in ~2 RTT), listing cache with prefetch, optimistic UI.
* Transfers local ↔ VPS ↔ VPS with queue, pause, cancel, conflict sheet
  (Replace is staged, Merge, Keep Both, Skip).
* Media streaming with seeking, external players, thumbnails (freedesktop
  cache, EXIF, scaled JPEG decode, server-side ffmpeg), fixed thumbnail frames.
* Open remote files in local apps with save-back and conflict detection.
* Finder UI: list / icon / column views, tabs, dual pane, Quick Look,
  address bar, Home with live server stats, git status/history, terminal.

---

## Phase 4 — Windows and macOS, flawlessly

The code is written to be cross-platform, but **only Linux has been run**.
The requirement is that all three OSes work with no known issues.

**CI first.** GitHub Actions matrix (`ubuntu-latest`, `windows-latest`,
`macos-latest` universal) running `cargo test --workspace`, `pnpm check`,
`pnpm build` and `tauri build` on every push. This catches compile errors
on the other OSes long before anyone runs the app there.

**Windows**
* Verify drive roots (`C:/`, virtual `/`), hidden attribute, trash.
* ssh-agent: Windows OpenSSH named pipe **and** Pageant; Credential Manager.
* Terminal (Windows Terminal → `cmd` fallback), players (VLC, MPC-HC, PotPlayer
  default install paths), ffmpeg absent by default.
* Editors lock open files: edit sessions must retry uploads on sharing
  violations.
* WebView2 differences (fonts, scrollbars, media codecs).
* Window chrome: Windows-style controls option, Mica/Acrylic via
  `window-vibrancy`.
* Code signing (Azure Trusted Signing or an OV certificate) so SmartScreen
  doesn't warn.

**macOS**
* Native traffic lights (`titleBarStyle: Overlay`) and real vibrancy in the
  sidebar; `system-ui` font (SF Pro is licensed on Apple platforms).
* ⌘ shortcuts and Finder conventions (⌘⌫ trash, Return = rename, ⌘O open,
  ⌘↓ enter, ⌘[ / ⌘] history).
* Terminal.app / iTerm2 / Ghostty, IINA / VLC, Homebrew paths
  (`/opt/homebrew/bin`), macOS Keychain, `launchd` SSH_AUTH_SOCK.
* Optional: Quick Look thumbnails via `QLThumbnailGenerator` for local files.
* Universal binary (Apple Silicon + Intel), **signing + notarization**
  (Apple Developer account, $99/yr) — unsigned downloads are blocked.

**Shipping**
* `tauri-action` release workflow on version tags: Windows `.msi`/`.exe`,
  Linux `.deb`/`.rpm`/`.AppImage`, macOS `.dmg`, published to GitHub
  Releases.
* `tauri-plugin-updater` auto-update with signed update manifests.
* Estimate: CI ≈ 1 day; Windows hardening 2–5 days; macOS 1–2 weeks with
  real-hardware testing.

---

## Phase 5 — Cloud storage

Requested: **iCloud Drive, Google Drive, Yandex Disk** "and similar". Every
cloud becomes a `vpsfiles_vfs::Backend`, so browsing, transfers (cloud ↔ VPS ↔
local), Quick Look, thumbnails and edit sessions work unchanged.

Target providers: Google Drive (incl. shared drives), iCloud Drive, Yandex
Disk, OneDrive, Dropbox, Mail.ru Cloud, Mega, pCloud, Proton Drive, Box;
object storage (S3, Cloudflare R2, MinIO, Backblaze B2, Wasabi); WebDAV
(Nextcloud, ownCloud); SMB / NAS shares; FTP / FTPS.

### Architecture: two tiers

**Tier A — rclone engine, for breadth.** rclone (60k★) already speaks 70+
providers, including `drive`, `iclouddrive`, `yandex`, `onedrive`,
`dropbox`, `mailru`, `mega`, `pcloud`, `protondrive`, `s3`, `webdav`,
`smb`, `ftp`.
* Not bundled (it's ~60 MB and would double the app). It's downloaded and
  checksum-verified on first "Add Cloud Account".
* It runs as `rclone rcd` on loopback with a random auth token.
* Metadata ops map onto rc endpoints (`operations/list`, `stat`, `mkdir`,
  `deletefile`, `purge`, `movefile`, `copyfile`).
* Byte streams (reads with Range, writes) go through a per-remote
  `rclone serve http/webdav` on loopback.
* OAuth runs through rclone's browser flow. Tokens live in the OS
  keychain, never in plain config files.

**Tier B — native backends, for the big four** (Google Drive, Yandex Disk,
OneDrive, Dropbox), where the UX gains are real:
* the provider's own thumbnails (`thumbnailLink`, Yandex `preview`) instead
  of downloading images;
* change feeds (Drive `changes`, Dropbox `list_folder/longpoll`, Graph
  `delta`) for live folder updates without polling;
* share links, trash/restore, version history, storage quota on Home;
* Google Docs/Sheets export formats, shortcuts, shared drives.

### iCloud specifics

There is no public iCloud Drive API.
* **macOS:** the local `~/Library/Mobile Documents/com~apple~CloudDocs`
  folder is first-class, so it works natively and offline.
* **Windows:** detect the iCloud for Windows folder (`%USERPROFILE%\iCloudDrive`).
* **Linux:** rclone `iclouddrive`. It needs a trusted 2FA session and
  Advanced Data Protection turned off. Ship it marked experimental.

### UX

* "Add Cloud Account" sheet with provider logos; a Cloud section in the
  sidebar; cloud cards on Home with quota bars.
* "Copy link", "Open in browser", "Make available offline" (later).

---

## Phase 6 — Power features

Ideas collected from the strongest open-source file managers (sources
below), filtered for a server- and cloud-centric manager.

### Server tools (our niche — nobody does these well in a Finder UI)
* **Direct VPS → VPS copy** (run `rsync`/`scp` on the source host) and
  **rsync delta transfers** when both ends have rsync.
* **Folder compare & sync** (local ↔ server, server ↔ server; mirror or two-way,
  preview before applying) — WinSCP / ForkLift style.
* **Resume interrupted transfers**, checksum verification, bandwidth limit,
  scheduled transfers / backups.
* **Mount a server or cloud as a real drive** so any app can use it
  (FUSE / macFUSE / WinFsp) — Mountain Duck style.
* **Read-only mode per server** ("production safety"): writes need an
  explicit unlock, with an audit log of every write we made.
* Port-forwarding manager (expose a remote DB/admin port on localhost).
* Live log viewer (`tail -f`, filter, highlight) for remote files.
* Processes, `systemctl` services, `docker ps` — and **Docker containers as
  browsable sources** (`docker exec` on the server).
* Disk usage analyzer (treemap from `du`, works remotely).
* Remote archive compress/extract, checksums (`sha256sum`), chmod/chown
  editor — all executed on the server, no download.
* SSH key manager: generate keys, install public key (`ssh-copy-id`),
  known_hosts viewer.
* Broadcast: upload a file / run a command on several servers at once.
* Remote trash (move to `~/.local/share/Trash` on the server) with undo.

### Search and organisation
* **Global search** across local, servers and clouds (`fd`/`ripgrep` locally,
  `find`/`rg` over exec remotely), typo-tolerant ranking (Sigma), content
  search, saved searches / smart folders.
* An index à la Spacedrive: live, content-identified (BLAKE3), so search,
  **duplicate finder** and "where else does this file exist?" work across
  every location, even offline ones.
* Tags and colours (Files, Finder), item filters by glob / size / date /
  MIME (Sigma), recents and frecency jump (zoxide-style).

### Productivity
* **Command palette** (Ctrl/⌘+K) for every action (Sigma, Files).
* Bulk rename with live preview, regex and counters (yazi).
* Built-in quick editor (CodeMirror) for small remote edits, and a **diff
  viewer** (local vs remote, server vs server).
* Undo / redo of file operations.
* Shelf: a staging area to collect files from many folders, then drop
  them all at once (Files).
* Richer Quick Look: PDF, archives as folders, code with syntax
  highlighting (yazi).
* Gallery / slideshow and video playlists.
* LAN sharing with a QR code (Sigma).

### Git, beyond read-only
Fetch / pull / push, switch branch, per-file diff and blame. Confirm every
write when the repo is on a server.

### Platform
* Plugin / extension API (yazi's Lua plugins and Files' extensions show the
  demand).
* Themes; **UI languages: Uzbek, Russian, English**.
* "Set as default file manager" (Windows, Linux).
* Portable mode; settings sync through a connected cloud account.

---

## Suggested order for the next sessions

1. CI matrix + Windows hardening (unblocks "works on every OS").
2. Resume / verify transfers + direct VPS → VPS + folder compare & sync.
3. Cloud Tier A (rclone engine) → Google Drive, Yandex Disk, iCloud first.
4. Global search + command palette + i18n (uz / ru / en).
5. macOS polish, signing, notarization; installers + auto-update on all OSes.
6. Cloud Tier B for the big four; mount as drive; plugins.

## Sources

* [files-community/Files](https://github.com/files-community/Files) — git
  integration, tags, shelf, dual pane, status center
* [spacedriveapp/spacedrive](https://github.com/spacedriveapp/spacedrive) —
  live index across local/remote/cloud, BLAKE3 identity, duplicates
* [aleksey-hoffman/sigma-file-manager](https://github.com/aleksey-hoffman/sigma-file-manager)
  — typo-tolerant global search, command palette, LAN sharing with QR, filters
* [sxyazi/yazi](https://github.com/sxyazi/yazi) — async task scheduling with
  priorities, preloading, plugins, bulk rename, rclone/git plugins
* [rclone/rclone](https://github.com/rclone/rclone) — 70+ storage backends
  incl. Google Drive, iCloud Drive, Yandex Disk
