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
