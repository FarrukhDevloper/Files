# Files

A fast, lightweight, Finder-style file manager for your computer **and your
servers**. Every host in `~/.ssh/config` shows up in the sidebar. You can browse
it like a local disk, copy between any two places (local ↔ VPS, VPS ↔ VPS) and
play videos straight from the server.

Built with **Tauri 2 + Rust** (backend) and **Svelte 5** (UI). Targets Linux
and Windows first, macOS later.

## Features

- Local disk, drives and trash, plus every SSH host from `~/.ssh/config`
  (Include, `Host *` defaults, ssh-agent, keys, passwords, hashed
  `known_hosts`)
- SFTP browsing with automatic reconnect
- Transfer queue for copy/move between any two sources, with pause, resume,
  cancel and live progress. Conflicts can be resolved with Replace, Merge,
  Keep Both or Skip; Replace is staged, so the original survives a failed copy
- Video and audio playback with seeking, streamed from a loopback HTTP server
  (no full download)
- Finder-style UI: List, Icon and Column views; tabs; drag & drop; Quick Look;
  thumbnails; full keyboard control; light and dark themes
- Dual pane, git status and history, live server stats, Open in Terminal
- Light: about 14 MB binary, ~135 MB RAM, ~0 % CPU when idle

## Develop

Requirements: Rust (stable), Node 20+, pnpm. On Linux you also need the
[Tauri system packages](https://v2.tauri.app/start/prerequisites/)
(`libwebkit2gtk-4.1-dev`, …).

```bash
pnpm install
pnpm dev           # UI only, in the browser, with a mock backend
pnpm tauri dev     # the full desktop app
cargo test --workspace
```

Architecture, the IPC contract and the design brief are in [`docs/`](docs/).

## Status

Runs on Linux today. Windows and macOS hardening, cloud storage (Google
Drive, iCloud Drive, Yandex Disk, OneDrive, Dropbox, S3, WebDAV, …) and
installers for every OS are next. See [docs/ROADMAP.md](docs/ROADMAP.md).
