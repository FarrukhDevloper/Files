# UI design brief — Finder-style, light and fast

Goal: feels like macOS Finder on Linux and Windows. Original artwork only —
**do not copy Apple assets** (SF Pro, SF Symbols, Finder folder icons are
licensed for Apple platforms only). Use Inter Variable + Lucide, and draw
our own folder/file icons as inline SVG.

## Window

```
┌─────────────────────────────────────────────────────────────────────────┐
│ ● ● ●   ‹  ›   Downloads          [▦ ☰ ▥]   [⋯]   [⇅ 2]   ( 🔍 Search ) │  toolbar = drag region
├──────────────┬──────────────────────────────────────────────────────────┤
│ Favourites   │ Name                    ▾ Date Modified     Size   Kind  │
│  ⌂ Home      │ 📁 Projects               Today, 14:02        —    Folder │
│  ▭ Desktop   │ 📄 invoice.pdf            9 Oct 2026     98 KB    PDF    │
│  ↓ Downloads │ 🎬 trailer.mp4            1 Nov 2026     52 MB    Video  │
│ Locations    │                                                          │
│  🖥 Computer  │                                                          │
│ Servers      │                                                          │
│  ● web-1     │                                                          │
│  ○ db-1      │                                                          │
├──────────────┴──────────────────────────────────────────────────────────┤
│ 🖥 Computer › home › me › Downloads                3 items, 120 GB free  │  path bar
└─────────────────────────────────────────────────────────────────────────┘
```

* Frameless window (`decorations: false`). Toolbar is `data-tauri-drag-region`.
  Traffic lights left (close / minimize / zoom) via `@tauri-apps/api/window`.
  Double-click toolbar = toggle maximize.
* Sidebar ~220 px, resizable, translucent-looking tint (solid colour — no
  heavy blur; WebKitGTK is slow with `backdrop-filter`).
* Server rows show a status dot: grey disconnected, amber connecting,
  green connected, red failed (tooltip = error). Click = connect + open home.

## Views

* **List** (default): sortable columns Name / Date Modified / Size / Kind,
  zebra striping, folders first. Must be virtualized (only visible rows in
  the DOM) — 10k entries should scroll at 60 fps.
* **Icons**: grid of 64 px icons with two-line names.
* **Columns** (Miller columns): later milestone, keep layout pluggable.

## Interaction (Ctrl on Win/Linux, ⌘ on macOS)

| Action | Keys |
|---|---|
| Open | double-click, Enter, Ctrl+↓ |
| Parent | Backspace, Ctrl+↑ |
| Back / forward | Alt+← / Alt+→, mouse buttons 4/5 |
| Quick Look | Space |
| Rename | F2, or click name of selected item |
| New folder | Ctrl+Shift+N |
| Copy / cut / paste | Ctrl+C / X / V — paste across sources starts a transfer |
| Delete | Delete (local → trash; remote → confirm dialog, permanent) |
| Select all | Ctrl+A |
| Go to path | Ctrl+L |
| Views | Ctrl+1 / 2 / 3 |
| Hidden files | Ctrl+H |

Selection: click, Ctrl+click toggle, Shift+click range, rubber-band later.
Context menu (custom HTML, Finder-like): Open, Quick Look, Get Info, Rename,
Copy, Cut, Paste, New Folder, Move to Trash / Delete.

## Quick Look

Space opens a centered overlay: images (`<img>`), video/audio (`<video>` /
`<audio>` with `api.mediaUrl`), text/code (`api.readText`, monospaced),
otherwise a big icon + metadata. Space/Esc closes, ←/→ moves selection.

## Transfers

Toolbar button with a count badge opens a popover: one row per job — name,
"12.4 MB of 52 MB — 8.1 MB/s — 4 s left", progress bar, pause/resume,
cancel (×). Finished rows fade, "Clear" button.

## Prompts

`prompt:request` opens a sheet (slides down from the toolbar, Finder-style):
* `hostKey`: "The authenticity of host … can't be established" + fingerprint,
  [Cancel] [Trust & Connect].
* `password` / `passphrase`: secure field, [Cancel] [Connect].

## Tokens (`src/styles/tokens.css`)

Light + dark via `prefers-color-scheme` and `[data-theme]` override.
Accent `#0a84ff` / dark `#0a84ff`, selection = accent; unfocused-window
selection = grey. Radius 6 (controls) / 10 (popovers). Font 13 px UI,
11 px secondary. Row height 24 px list, 22 px sidebar.
