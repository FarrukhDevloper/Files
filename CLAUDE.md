# VPS Files

Finder-style desktop file manager for the local disk + every host in
`~/.ssh/config`. Tauri 2 + Rust backend, Svelte 5 frontend. Must stay light
(idle CPU ≈ 0 %, low RAM).

Read before working:
- docs/ARCHITECTURE.md — crates, data flow, rules
- docs/IPC.md — the command/event contract between Rust and the UI
- docs/TASKS.md — **who owns which paths** (several agents work here at once)
- docs/DESIGN.md — UI brief

Commands:
- `cargo test -p <crate>` / `cargo check --workspace`
- `pnpm dev` (UI in browser with mock backend), `pnpm check`, `pnpm build`
- `pnpm tauri dev` (full app; lead only while other agents are active)

Conventions:
- Paths: absolute, `/`-separated, see `crates/vfs/src/path.rs`.
- Errors: `VfsError` → `{ code, message }` across IPC.
- UI calls the backend only through `src/lib/ipc/api.ts`.
- Do not commit; the git root is the parent folder.
- The SSH hosts in `~/.ssh/config` are production servers: never write to them.
