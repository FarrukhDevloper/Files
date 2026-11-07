# Work split — 3 agents, one working tree

Three Claude sessions work in this directory at the same time (herdr tab
`w2:t1`). Ownership is by path so nobody edits the same file.

| Agent | herdr pane | Owns |
|---|---|---|
| **lead** (session `vps-files-95`) | `w2:p1` | `crates/vfs/`, `crates/transfer/`, `src-tauri/`, `docs/`, `Cargo.toml`, `src/lib/ipc/types.ts`, `src/lib/ipc/api.ts` |
| **ssh** | `w2:p2` | `crates/ssh/`, `crates/stream/` |
| **ui** | `w2:p3` | everything under `src/` except `types.ts`/`api.ts`; `package.json` frontend deps |

## Rules for everyone

1. Stay inside your paths. Need a change elsewhere (new command, new field,
   trait change)? **SendMessage to the lead session `vps-files-95`** with the
   exact proposal; don't edit the contract yourself. Lead updates Rust +
   TS + docs together.
2. Keep the public API in your crate's `lib.rs` stable — the app already
   compiles against it. Adding is fine; renaming/removing needs the lead.
3. Build and test only what you own:
   * ssh: `cargo test -p vpsfiles-ssh -p vpsfiles-stream`, `cargo clippy -p …`
   * ui: `pnpm check && pnpm build`
   * Before reporting done, `cargo check --workspace` must still pass.
   The cargo `target/` dir is shared; cargo serializes builds with a lock —
   "Blocking waiting for file lock" is normal, just wait.
4. **Never write to real servers.** Every host in the developer's
   `~/.ssh/config` is production. Read-only checks
   (connect, list a directory) are OK only if needed; anything that creates,
   modifies or deletes remote files must use a local/in-process test server.
   Never modify `~/.ssh/known_hosts` or `~/.ssh/config` from tests.
5. No `git commit` / `git add` — the git root is the parent folder and holds
   unrelated projects.
6. New dependencies: allowed when they clearly pay for themselves; prefer
   small crates/packages. Add Rust deps to the workspace
   `[workspace.dependencies]` only via the lead; crate-local dev-deps are fine.
7. When a milestone is done: SendMessage the lead a 3–6 line summary (what
   works, how it was tested, what's left/blocked).

---

## ssh agent

Read `crates/vfs/src/backend.rs` (the trait), `crates/ssh/src/*`,
`crates/stream/src/lib.rs`, docs/ARCHITECTURE.md §SSH/§Media.

1. **config.rs — `load_hosts_from`**
   * `Host a b` → two entries; wildcard/negated patterns (`*`, `?`, `!`) are
     never listed but their options apply to matching aliases, first value
     wins (OpenSSH semantics). `Include` (relative to `~/.ssh/`, globs).
   * Defaults: `hostname = alias`, `port = 22`, `~` expanded in
     `IdentityFile`. Missing file → empty list.
   * Done with an own parser (ssh2-config misapplied `Match` blocks).
2. **manager.rs — `SshManager`**
   * `HashMap<alias, Connection>`; connects on `backend()`; concurrent
     callers for one alias share one attempt.
   * Status changes → `ConnEvent` broadcast; `status()` reflects them.
   * Auth: ssh-agent (`SSH_AUTH_SOCK`; Windows OpenSSH agent pipe if russh
     supports it) → IdentityFiles (encrypted → `Prompter::passphrase`) →
     `Prompter::password`. Default identity files when none configured
     (`id_ed25519`, `id_ecdsa`, `id_rsa`).
   * known_hosts: unknown → `Prompter::confirm_host_key` → append on accept;
     mismatch → `VfsError::HostKey`, no prompt. Handle `[host]:port` entries
     and hashed hostnames. Make the known_hosts path injectable for tests
     (e.g. `SshManager::with_options`), default `~/.ssh/known_hosts`.
   * Keepalive (~30 s); a dropped connection → `disconnected`, next call
     reconnects. `disconnect()` closes cleanly.
   * `exec(alias, cmd)` → `ExecOutput`.
   * ProxyJump: not now — return `VfsError::Unsupported` with a clear message
     if a host needs it.
3. **sftp.rs — `SftpBackend`** implements `vpsfiles_vfs::Backend` fully.
   Map SFTP status codes to `VfsError`. `home()` = canonicalize("."). Hidden =
   dotfile. `capabilities().exec = true`. `rename` must fail with
   `AlreadyExists` if the target exists. Listing a 10k-entry dir must not
   do 10k round-trips (use the readdir attrs, only stat symlinks).
4. **Tests**: prefer an in-process russh server + russh-sftp server over a
   temp dir (fully hermetic). If that is too heavy, at least config tests
   plus an `#[ignore]` test driven by `VPSFILES_TEST_HOST` that is read-only.
5. **stream crate — `MediaServer`**: axum on `127.0.0.1:0`, random token,
   `GET`/`HEAD`, single `Range: bytes=a-b` / `a-` / `-n` → 206 with
   `Content-Range`, no range → 200, bad range → 416, `Accept-Ranges: bytes`,
   `Content-Type` via `mime_guess`, percent-decoded path, wrong token → 404.
   Fix `url_for` to percent-encode segments. Tests against `LocalBackend`.

## ui agent

Read docs/DESIGN.md (the brief), docs/IPC.md, `src/lib/ipc/*`.
Develop in the browser against the mock: `pnpm dev` → http://localhost:5173
(you own the dev server on port 5173). Import only from `$lib/ipc/api`.

1. Tokens + base styles (light/dark), window chrome with traffic lights and
   drag region (no-ops in the browser).
2. Stores (Svelte 5 runes, `*.svelte.ts`): hosts + connection status,
   per-tab browser state (location, history back/forward, entries, sort,
   view mode, selection, show hidden), transfers, prompts, toasts.
3. Sidebar (Favourites from `localPlaces`, Locations, Servers with status
   dots), toolbar, path bar with breadcrumbs, status text.
4. List view (virtualized, sortable, folders first) and Icon view.
5. Keyboard + mouse per DESIGN.md, custom context menu, inline rename,
   new folder, delete (confirm for remote), copy/cut/paste → `transferStart`.
6. Transfers popover, Quick Look (image/video/audio/text), prompt sheet,
   error toasts (by `VfsError.code`).
7. File-type icons as our own inline SVGs (folder, generic doc, image,
   video, audio, archive, code, pdf, disk image).

Lightness: no idle timers/animation loops; virtualize; avoid heavy
`backdrop-filter`; keep the bundle small (no UI kit).

## lead

* `crates/transfer`: real `TransferManager` (queue, relay copy any→any,
  move, conflicts, pause/resume/cancel, progress throttling, tests).
* Integration in `src-tauri`, `pnpm tauri dev` on Linux, contract changes,
  reviews of the other two agents' work.

---

# Round 2 — "maximum convenience" (user feedback, 2026-11-07)

User asked for: bigger UI, a Linux/Windows-style address bar at the top,
opening remote files in the laptop's own apps (editors, players, image
viewers) with changes synced back, macOS-style thumbnails everywhere (incl.
video), a home page with servers as cards, and adding/configuring servers in
the app. Contract is already updated in `src/lib/ipc/{types,api,media}.ts`
and docs/IPC.md; Rust commands land as the backend pieces do.

## ssh agent (round 2)

1. **App-managed hosts** in `crates/ssh`:
   * `HostConfig` gains `origin: HostOrigin` (`SshConfig` | `App`, serde
     camelCase → `"sshConfig"` / `"app"`). Listing = ssh-config hosts + app
     hosts (`dirs::config_dir()/vps-files/hosts.json`, written atomically);
     an app alias that collides with an ssh-config alias is rejected
     (`already_exists`).
   * `pub struct HostDraft { alias, hostname, user: Option<String>, port: u16,
     identity_file: Option<PathBuf>, password: Option<String> }`
     (Deserialize, camelCase) and on `SshManager`:
     `save_host(draft, previous_alias: Option<&str>) -> Result<HostConfig>`,
     `remove_host(alias) -> Result<()>`, `forget_password(alias) -> Result<()>`.
     Renaming/editing a connected host disconnects it.
   * Store paths injectable via `SshOptions` for tests.
2. **Keychain** (`keyring` is in the workspace): passwords and key
   passphrases saved per host; auth tries a saved secret before prompting;
   a saved secret that fails auth is deleted and the user is prompted.
   Change `Prompter::password/passphrase` to return
   `Option<Secret { value: String, remember: bool }>`. You may edit
   `src-tauri/src/prompter.rs` and the `prompt_respond` command in
   `src-tauri/src/commands.rs` for exactly this change (add a
   `remember: bool` arg). Linux without a Secret Service must degrade to
   "not remembered", never fail the connection.
3. **Thumbnails** in `crates/stream`: `GET /{token}/thumb/{size}/{source}/{seg}…`
   (size clamped to 32..=512). Images via `image` (jpeg/png/webp/gif/bmp):
   decode, `thumbnail()` resize, encode JPEG (PNG if alpha). Videos: if
   `ffmpeg` is on PATH, grab one frame at ~10 % / max 5 s by pointing ffmpeg
   at our own media URL (Range makes remote cheap), 15 s timeout. Disk cache
   `dirs::cache_dir()/vps-files/thumbs/<hash(source,path,size,mtime)>`,
   `Cache-Control: max-age=31536000, immutable` (UI adds `?v=mtime`).
   Limits: ≤ 2 concurrent decodes, remote images ≤ 25 MB, local ≤ 100 MB,
   otherwise 404 quickly. Add `MediaServer::base_url() -> String`
   (`http://127.0.0.1:PORT/TOKEN`, no trailing slash).
4. **ProxyJump** (user approved): single + chained, as briefed earlier.

## ui agent (round 2)

1. **Bigger UI + zoom**: default base font 14 px and row heights scaled to
   match; Preferences "Interface size" (Small 90 % / Default 100 % / Large
   115 % / Larger 130 %) + Ctrl+= / Ctrl+- / Ctrl+0. In Tauri use
   `getCurrentWebview().setZoom()` (permission granted); in the browser fall
   back to CSS `zoom`. Persist.
2. **Address bar at the top** (GNOME Files / Explorer style): breadcrumb
   buttons in the toolbar; click on empty space / Ctrl+L turns it into an
   editable text field with the full path (remote shows a server chip
   `tanla:` before the path); Enter navigates, Esc restores, Tab
   completes folder names from the parent listing. The bottom path bar
   becomes optional (Preferences, default off).
3. **Home page**: a start page (sidebar item "Home" at the top, default for
   new windows/tabs, Preferences option) with "This computer" (places +
   drives with usage bars from `diskFree`) and "Servers" as cards (server
   icon, alias, user@host:port, status dot, Connect/Open, disk usage when
   connected) plus an "Add Server" card.
4. **Server management**: Add/Edit Server sheet (`saveHost`): alias, host,
   user, port, auth = Key file (picker via `@tauri-apps/plugin-dialog`
   `open()`, permission granted) / Password (saved to keychain) / ssh-agent.
   Validate alias. Sidebar + card context menu: Connect, Disconnect, Edit
   (app hosts) / "Edit ~/.ssh/config" (`openExternal` on `sshConfigPath()`)
   for config hosts, Forget saved password, Remove (app hosts), Copy
   address. Password/passphrase prompt sheet gets "Remember in keychain"
   (`promptRespond(..., remember)`).
5. **Open in local apps**: double-click / Enter on a remote file calls
   `openExternal` (confirm first above 200 MB) — it downloads, opens in
   the OS default app and syncs saves back. Toolbar "Editing" indicator
   (popover of `editList()` / `edit:update` sessions with state chips and
   Stop), toasts on upload, conflict sheet → `editResolve`. Media: context
   menu "Open With" → Default App · Quick Look · each of
   `externalPlayers()` (`openInPlayer`); for remote video/audio
   double-click prefers an external player when one exists, else Quick Look.
6. **Thumbnails everywhere** via `src/lib/ipc/media.ts` (`initMedia()` once,
   then `thumbUrl(loc, size, modified)`): list rows (icon-sized), icon
   view, column view + info pane, Quick Look poster; images and videos.
   `thumbUrl` returns null in the browser mock → keep your current
   fallback there. Keep the 4-concurrent / lazy rules.

## lead (round 2)

* `crates/edit`: remote edit sessions (download to cache, open, watch with
  `notify`, debounced upload with conflict detection, resolve, stop).
* Commands: hosts CRUD, edit_*, media_base, external_players /
  open_in_player (mpv, vlc, celluloid… on PATH; VLC default path on
  Windows), window-state plugin, contract docs.
