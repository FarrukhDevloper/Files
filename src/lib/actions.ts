// User-level file operations shared by the keyboard, the context menu, the
// toolbar and the views. Each one reports failures as toasts.

import { tick } from "svelte";
import { api } from "$lib/ipc/api";
import type { Entry, HostInfo, Location, Place, SourceId, TransferOp } from "$lib/ipc/types";
import type { Browser } from "$lib/stores/browser.svelte";
import { clipboard } from "$lib/stores/clipboard.svelte";
import { edits } from "$lib/stores/edits.svelte";
import type { DropTarget } from "$lib/stores/dnd.svelte";
import { hosts } from "$lib/stores/hosts.svelte";
import { listings } from "$lib/stores/listings";
import { ICON_SIZES, prefs } from "$lib/stores/prefs.svelte";
import { tabs } from "$lib/stores/tabs.svelte";
import { toasts } from "$lib/stores/toasts.svelte";
import { transfers } from "$lib/stores/transfers.svelte";
import { ui, type ConflictAction, type MenuItem } from "$lib/stores/ui.svelte";
import { previewKind } from "$lib/util/filetype";
import { formatSize, plural } from "$lib/util/format";
import { ancestors, baseName, isRoot, joinPath, parentPath, sameLocation, sameSource, sourceKey, sourceLabel } from "$lib/util/path";
import { isMac, shortcutLabel } from "$lib/util/platform";

const local: SourceId = { kind: "local" };

// ---- navigation ----

export async function openPlace(b: Browser, place: Place) {
  await b.go({ source: local, path: place.path });
}

/** Sidebar server click: connect if needed, then open the remote home —
 *  unless the user went somewhere else while it was connecting. */
export async function openHost(b: Browser, alias: string) {
  const source: SourceId = { kind: "ssh", host: alias };
  const token = b.claimNav();
  try {
    await hosts.connect(alias);
  } catch (e) {
    if (b.isCurrentNav(token)) toasts.error(e, `Couldn't connect to “${alias}”`, { label: "Retry", run: () => void openHost(b, alias) });
    return;
  }
  if (!b.isCurrentNav(token)) return;
  try {
    const home = await api.homeDir(source);
    if (b.isCurrentNav(token)) await b.go({ source, path: home });
  } catch (e) {
    if (b.isCurrentNav(token)) toasts.error(e, `Couldn't open “${alias}”`);
  }
}

/** What a new window or tab shows first (Preferences → start page). */
export function openStart(b: Browser) {
  if (prefs.startPage === "home") b.showHome();
  else void openLocalHome(b);
}

export function newTab(from: Browser = tabs.current) {
  openStart(tabs.open(from));
}

/** F3: two independent panes side by side; the new right pane starts in
 *  the same folder. Again: back to one pane. */
export function toggleDualPane() {
  const from = tabs.primary;
  const b = tabs.toggleSplit();
  if (b) {
    if (from.location) void b.go(from.location, { history: "none" });
    else b.showHome({ history: "none" });
  }
  void tick().then(() => ui.focusFiles());
}

/** Dual pane F5 / F6: copy / move the selection into the other pane's folder. */
export async function transferToOtherPane(op: TransferOp) {
  const b = tabs.current;
  const dest = tabs.other?.location;
  const sel = b.selection;
  if (!dest || !b.location || !sel.length) return;
  if (sameLocation(b.location, dest)) {
    toasts.info("Both panes show the same folder", "Open another folder in the other pane first.");
    return;
  }
  await startTransfer(op, sel.map((e) => b.locationOf(e)), dest);
}

export async function openLocalHome(b: Browser) {
  try {
    const home = await api.homeDir(local);
    await b.go({ source: local, path: home });
  } catch (e) {
    await b.go({ source: local, path: "/" });
    toasts.error(e, "Couldn't find your home folder");
  }
}

/** Above this, opening a remote file in an app asks first (it downloads). */
const BIG_DOWNLOAD = 200 * 1000 * 1000;

/** Enter / double-click. Folders navigate. Files open in the OS default app:
 *  local ones in place, remote ones as a synced temp copy. Remote audio and
 *  video stream into an external player instead, else into Quick Look. */
export async function openEntry(b: Browser, entry: Entry) {
  const loc = b.locationOf(entry);
  if (entry.kind === "dir") {
    await b.go(loc);
    return;
  }
  const kind = previewKind(entry);
  if (loc.source.kind === "ssh" && (kind === "video" || kind === "audio")) {
    if (edits.players.length) {
      await openInPlayer(entry, loc, null);
    } else {
      b.selectOnly(entry.path);
      ui.quickLook = true;
    }
    return;
  }
  await openWithDefaultApp(entry, loc);
}

export async function openWithDefaultApp(entry: Entry, loc: Location) {
  if (loc.source.kind === "ssh" && entry.size > BIG_DOWNLOAD) {
    const ok = await ui.confirm({
      title: `Download “${entry.name}” to open it?`,
      message: `It is ${formatSize(entry.size)}. A copy is downloaded from ${loc.source.host} and opened in its app; your saves are uploaded back.`,
      confirm: "Download & Open",
    });
    if (!ok) return;
  }
  try {
    if (loc.source.kind === "local") await api.openExternal(loc);
    else await edits.start(loc);
  } catch (e) {
    toasts.error(e, `Couldn't open “${entry.name}”`);
  }
}

/** Stream into an external player (`null` = the first one found). */
export async function openInPlayer(entry: Entry, loc: Location, player: string | null) {
  try {
    await api.openInPlayer(loc, player);
  } catch (e) {
    toasts.error(e, `Couldn't play “${entry.name}”`);
  }
}

/** "vlc" → "VLC"; unknown names keep their spelling. */
export function playerLabel(player: string): string {
  const known: Record<string, string> = { vlc: "VLC", mpv: "mpv", iina: "IINA", celluloid: "Celluloid", totem: "Videos", smplayer: "SMPlayer", haruna: "Haruna" };
  return known[player.toLowerCase()] ?? player;
}

/** "Open with …" items for one file: the default app and each player. */
function openWithItems(b: Browser, entry: Entry): MenuItem[] {
  if (entry.kind !== "file") return [];
  const loc = b.locationOf(entry);
  const kind = previewKind(entry);
  const media = kind === "video" || kind === "audio";
  const items: MenuItem[] = [];
  // Plain "Open" already uses the default app, except for remote media.
  if (media && loc.source.kind === "ssh") {
    items.push({ label: "Open with Default App", run: () => void openWithDefaultApp(entry, loc) });
  }
  if (media) {
    for (const p of edits.players) items.push({ label: `Open with ${playerLabel(p)}`, run: () => void openInPlayer(entry, loc, p) });
  }
  return items;
}

export async function openSelection(b: Browser) {
  const sel = b.selection;
  if (sel.length === 1) return openEntry(b, sel[0]!);
  // Several items: open files, ignore folders (Finder would open windows).
  for (const e of sel) if (e.kind !== "dir") await openEntry(b, e);
}

// ---- edit ----

export function startRename(b: Browser) {
  const sel = b.selection;
  if (sel.length !== 1) return;
  b.renaming = sel[0]!.path;
  b.revealPath(sel[0]!.path);
}

/** Folders being created (optimistic): a rename waits for the real one. */
const creating = new Map<string, Promise<Entry | null>>();

/** Optimistic: the new name shows at once and reverts if the server says no. */
export async function commitRename(b: Browser, entry: Entry, newName: string) {
  b.renaming = null;
  const name = newName.trim();
  const dir = b.location;
  if (!name || name === entry.name || !dir) return;
  const showing = () => sameLocation(b.location, dir);
  const shown: Entry = { ...entry, name, path: joinPath(dir.path, name), isHidden: name.startsWith(".") };
  b.replaceEntry(entry.path, shown, dir);
  if (showing()) b.selectOnly(shown.path);
  try {
    const key = `${sourceKey(dir.source)}|${entry.path}`;
    if (creating.has(key) && !(await creating.get(key))) {
      b.removePaths([shown.path], dir); // the folder was never created
      return;
    }
    const renamed = await api.renamePath({ source: dir.source, path: entry.path }, name);
    b.replaceEntry(shown.path, renamed, dir);
  } catch (e) {
    b.replaceEntry(shown.path, entry, dir);
    if (showing()) b.selectOnly(entry.path);
    toasts.error(e, `Couldn't rename “${entry.name}”`);
  }
}

/** Optimistic: the folder appears in rename mode at once; it's created in
 *  the background and removed again if that fails. */
export async function newFolder(b: Browser) {
  const dir = b.location;
  if (!dir || b.error) return;
  const taken = new Set(b.entries.map((e) => e.name.toLocaleLowerCase()));
  let name = "untitled folder";
  for (let n = 2; taken.has(name); n++) name = `untitled folder ${n}`;
  const path = joinPath(dir.path, name);
  const placeholder: Entry = {
    name, path, kind: "dir", size: 0, modified: Date.now(), mode: 0o755, isSymlink: false, isHidden: false,
  };
  b.filter = "";
  b.addEntry(placeholder, dir);
  b.selectOnly(path);
  b.renaming = path;
  const key = `${sourceKey(dir.source)}|${path}`;
  const done = api.createDir({ source: dir.source, path }).then(
    (entry) => {
      // A rename may already have replaced the placeholder; then it owns the entry.
      if ((sameLocation(b.location, dir) ? b.entries : listings.get(dir) ?? []).some((e) => e === placeholder)) {
        b.replaceEntry(path, entry, dir);
      }
      return entry;
    },
    (e) => {
      if (b.renaming === path) b.renaming = null;
      b.removePaths([path], dir);
      toasts.error(e, "Couldn't create a folder");
      return null;
    },
  );
  creating.set(key, done);
  void done.finally(() => creating.delete(key));
}

/** Local → Trash (no confirm, like Finder). Remote or `permanent` → confirm. */
export async function deleteSelection(b: Browser, permanent = false) {
  const sel = b.selection;
  if (!b.location || sel.length === 0) return;
  const remote = b.location.source.kind === "ssh";
  const what = sel.length === 1 ? `“${sel[0]!.name}”` : plural(sel.length, "item");
  if ((remote || permanent) && prefs.confirmDelete) {
    const source = b.location.source;
    const ok = await ui.confirm({
      title: `Delete ${what} permanently?`,
      message: source.kind === "ssh"
        ? `Items on ${source.host} are deleted immediately. This can't be undone.`
        : "The items are deleted immediately instead of going to the Trash. This can't be undone.",
      confirm: "Delete",
      danger: true,
    });
    if (!ok) return;
  }
  const dir = b.location;
  const locs = sel.map((e) => b.locationOf(e));
  // Optimistic: gone at once; a failure re-lists the folder.
  b.removePaths(sel.map((e) => e.path), dir, true);
  try {
    await api.deletePaths(locs, remote || permanent);
  } catch (e) {
    toasts.error(e, `Couldn't delete ${what}`);
    listings.drop(dir);
    for (const t of tabs.all) if (sameLocation(t.location, dir)) t.revalidate();
  }
}

// ---- clipboard / transfers ----

export function copySelection(b: Browser, op: "copy" | "move") {
  const sel = b.selection;
  if (sel.length === 0) return;
  clipboard.set(op, sel.map((e) => b.locationOf(e)));
}

export async function paste(b: Browser) {
  if (!b.location || clipboard.empty) return;
  const dest = b.location;
  const sources = clipboard.items;
  const op = clipboard.op;

  if (op === "move" && sources.every((s) => sameSource(s.source, dest.source) && parentPath(s.path) === dest.path)) {
    clipboard.clear();
    return;
  }
  const into = sources.find((s) => sameSource(s.source, dest.source) && ancestors(dest.path).includes(s.path));
  if (into) {
    toasts.push({ tone: "error", title: `Can't ${op} “${baseName(into.path)}” into itself` });
    return;
  }
  const started = await startTransfer(op, sources, dest);
  if (started && op === "move") clipboard.clear();
}

type Policy = Exclude<ConflictAction, "stop">;

/** Copy/move with Finder-style conflict questions. Lists `dest` once, asks
 *  per clashing name (optionally "apply to all"), then starts one transfer
 *  per chosen policy. Copies into their own folder are duplicated without
 *  asking. Resolves false if the user stopped or nothing was started. */
export async function startTransfer(op: TransferOp, sources: Location[], dest: Location): Promise<boolean> {
  // Windows volumes are case-insensitive; everything else compares exactly.
  const fold = dest.source.kind === "local" && /^[A-Za-z]:\//.test(dest.path);
  const norm = (n: string) => (fold ? n.toLocaleLowerCase() : n);

  let existing: Map<string, Entry>;
  try {
    // A cached listing is safe here: a clash it misses gets "keep both".
    existing = new Map((listings.get(dest) ?? (await listings.fetch(dest))).map((e) => [norm(e.name), e]));
  } catch {
    // Can't look first: let the backend keep both, the safe default.
    return (await transfers.start({ op, sources, destDir: dest, conflict: "keepBoth" })) !== null;
  }

  const groups: Record<Policy, Location[]> = { keepBoth: [], replace: [], merge: [], skip: [] };
  const clashes: { src: Location; existing: Entry }[] = [];
  for (const src of sources) {
    const hit = existing.get(norm(baseName(src.path)));
    const intoOwnFolder = sameSource(src.source, dest.source) && parentPath(src.path) === dest.path;
    if (!hit || intoOwnFolder) groups.keepBoth.push(src);
    else clashes.push({ src, existing: hit });
  }

  if (clashes.length) {
    const incoming = await Promise.all(clashes.map((c) => api.statPath(c.src).catch(() => null)));
    const folder = baseName(dest.path) || sourceLabel(dest.source);
    let forAll: Policy | null = null;
    for (const [i, c] of clashes.entries()) {
      const inc = incoming[i] ?? null;
      const allowReplace = !inc || (inc.kind === "dir") === (c.existing.kind === "dir");
      const allowMerge = inc?.kind === "dir" && c.existing.kind === "dir";
      const fits = (p: Policy) => (p !== "replace" || allowReplace) && (p !== "merge" || allowMerge);
      let choice: Policy;
      if (forAll && fits(forAll)) {
        choice = forAll;
      } else {
        const r = await ui.askConflict({
          name: c.existing.name, folder, op, allowReplace, allowMerge, existing: c.existing, incoming: inc,
          remaining: clashes.length - i,
        });
        if (r.action === "stop") return false;
        choice = r.action;
        if (r.all) forAll = choice;
      }
      groups[choice].push(c.src);
    }
  }

  showArriving(groups.keepBoth.filter((s) => !(sameSource(s.source, dest.source) && parentPath(s.path) === dest.path)), dest);
  let started = false;
  if (groups.keepBoth.length) started = (await transfers.start({ op, sources: groups.keepBoth, destDir: dest, conflict: "keepBoth" })) !== null || started;
  if (groups.replace.length) started = (await transfers.start({ op, sources: groups.replace, destDir: dest, conflict: "overwrite" })) !== null || started;
  if (groups.merge.length) started = (await transfers.start({ op, sources: groups.merge, destDir: dest, conflict: "merge" })) !== null || started;
  return started;
}

/** Items copied/moved into `dest` without a name clash appear there at once,
 *  dimmed; the job's end re-lists the folder (App: transfers.onSettled). */
function showArriving(sources: Location[], dest: Location) {
  if (!sources.length) return;
  const arriving = sources.map((src): Entry => {
    const name = baseName(src.path);
    const parent = parentPath(src.path);
    const known = parent !== null ? listings.get({ source: src.source, path: parent })?.find((e) => e.path === src.path) : undefined;
    return {
      name, path: joinPath(dest.path, name), kind: known?.kind ?? "file", size: known?.size ?? 0,
      modified: known?.modified ?? Date.now(), mode: known?.mode ?? null, isSymlink: false, isHidden: name.startsWith("."),
    };
  });
  for (const t of tabs.all) t.addPending(arriving, dest);
}

/** Drag & drop: same source moves, cross-source (or Ctrl/Alt) copies.
 *  A server target means its home folder. */
export async function dropItems(target: DropTarget, items: Location[], forceCopy: boolean) {
  let dest: Location;
  if (target.kind === "host") {
    const source: SourceId = { kind: "ssh", host: target.host };
    try {
      await hosts.connect(target.host);
      dest = { source, path: await api.homeDir(source) };
    } catch (e) {
      toasts.error(e, `Couldn't connect to “${target.host}”`);
      return;
    }
  } else {
    dest = target.location;
  }
  if (items.some((s) => sameSource(s.source, dest.source) && ancestors(dest.path).includes(s.path))) return;
  const op = !forceCopy && items.every((s) => sameSource(s.source, dest.source)) ? "move" : "copy";
  await startTransfer(op, items, dest);
}

export async function copyPath(b: Browser) {
  const sel = b.selection;
  const loc = b.location;
  if (!loc) return;
  const paths = sel.length ? sel.map((e) => e.path) : [loc.path];
  const prefix = loc.source.kind === "ssh" ? `${loc.source.host}:` : "";
  try {
    await navigator.clipboard.writeText(paths.map((p) => prefix + p).join("\n"));
  } catch {
    toasts.info("Couldn't access the system clipboard");
  }
}

// ---- panels ----

export function toggleQuickLook(b: Browser) {
  if (ui.quickLook) {
    ui.quickLook = false;
    return;
  }
  if (b.selection.length === 0) return;
  if (b.cursor === null || !b.selected.has(b.cursor)) b.cursor = b.selection[0]!.path;
  ui.quickLook = true;
}

export function getInfo(b: Browser, entry?: Entry) {
  const target = entry ?? b.selection[0];
  if (!target || !b.location) return;
  ui.sheet = { kind: "info", entry: target, location: b.locationOf(target) };
}

export function openPreferences() {
  ui.sheet = { kind: "prefs" };
}

/** Mod+L: edit the address bar. */
export function goToFolder() {
  ui.editAddress();
}

/** Address bar: absolute path, "~" = home of `source`. `token` comes from
 *  b.claimNav() when the caller already waited (e.g. for a connection). */
export async function goToPath(b: Browser, source: SourceId, typed: string, token = b.claimNav()): Promise<boolean> {
  let path = typed.trim();
  if (!path) return false;
  // Windows users paste "C:\Users\me"; the backend wants "C:/Users/me".
  if (source.kind === "local" && /^[A-Za-z]:(\\|$)/.test(path)) path = path.replaceAll("\\", "/").replace(/^([A-Za-z]:)$/, "$1/");
  if (path === "~" || path.startsWith("~/")) {
    try {
      path = joinPath(await api.homeDir(source), path.slice(2)).replace(/\/$/, "") || "/";
    } catch (e) {
      toasts.error(e, "Couldn't find the home folder");
      return false;
    }
    if (!b.isCurrentNav(token)) return false;
  }
  if (!isRoot(path)) path = path.replace(/\/+$/, "") || "/";
  const loc: Location = { source, path };
  try {
    const entry = await api.statPath(loc);
    if (!b.isCurrentNav(token)) return false;
    if (entry.kind === "dir") return b.go(loc);
    const parent = parentPath(path);
    return parent !== null && b.go({ source, path: parent }, { select: path });
  } catch (e) {
    toasts.error(e, `Couldn't go to “${typed.trim()}”`);
    return false;
  }
}

// ---- context menus ----

export function entryMenu(b: Browser, e: MouseEvent, entry: Entry) {
  if (!b.selected.has(entry.path)) b.selectOnly(entry.path);
  const sel = b.selection;
  const single = sel.length === 1;
  const remote = b.location?.source.kind === "ssh";
  const dir = single && sel[0]!.kind === "dir" ? sel[0]! : null;
  const items: MenuItem[] = [
    { label: "Open", shortcut: "Enter", run: () => void openSelection(b) },
    ...(single ? openWithItems(b, sel[0]!) : []),
    ...(dir ? [
      { label: "Open in New Tab", run: () => void tabs.open(b, b.locationOf(dir)) },
      { label: "Open in Terminal", run: () => void openTerminalAt(b.locationOf(dir)) },
    ] : []),
    { label: "Quick Look", shortcut: "Space", disabled: !single, run: () => toggleQuickLook(b) },
    { label: "Get Info", shortcut: shortcutLabel("Mod+I"), disabled: !single, run: () => getInfo(b) },
    "separator",
    { label: "Rename", shortcut: "F2", disabled: !single, run: () => startRename(b) },
    { label: single ? "Copy Path" : "Copy Paths", run: () => void copyPath(b) },
    "separator",
    { label: "Copy", shortcut: shortcutLabel("Mod+C"), run: () => copySelection(b, "copy") },
    { label: "Cut", shortcut: shortcutLabel("Mod+X"), run: () => copySelection(b, "move") },
    { label: pasteLabel(), shortcut: shortcutLabel("Mod+V"), disabled: clipboard.empty, run: () => void paste(b) },
    ...(tabs.split && tabs.other?.location
      ? ["separator" as const,
        { label: "Copy to Other Pane", shortcut: "F5", run: () => void transferToOtherPane("copy") },
        { label: "Move to Other Pane", shortcut: "F6", run: () => void transferToOtherPane("move") }]
      : []),
    "separator",
    { label: "New Folder", shortcut: shortcutLabel("Mod+Shift+N"), run: () => void newFolder(b) },
    "separator",
    remote
      ? { label: "Delete Permanently…", shortcut: deleteShortcut(), danger: true, run: () => void deleteSelection(b) }
      : { label: "Move to Trash", shortcut: deleteShortcut(), run: () => void deleteSelection(b) },
  ];
  ui.openMenu(e, items);
}

export function backgroundMenu(b: Browser, e: MouseEvent) {
  b.selectNone();
  const items: MenuItem[] = [
    { label: "New Folder", shortcut: shortcutLabel("Mod+Shift+N"), disabled: !b.location || !!b.error, run: () => void newFolder(b) },
    { label: pasteLabel(), shortcut: shortcutLabel("Mod+V"), disabled: clipboard.empty, run: () => void paste(b) },
    "separator",
    { label: "as Icons", checked: b.view === "icons", shortcut: shortcutLabel("Mod+1"), run: () => b.setView("icons") },
    { label: "as List", checked: b.view === "list", shortcut: shortcutLabel("Mod+2"), run: () => b.setView("list") },
    { label: "as Columns", checked: b.view === "columns", shortcut: shortcutLabel("Mod+3"), run: () => b.setView("columns") },
    "separator",
    ...([["name", "Name"], ["modified", "Date Modified"], ["size", "Size"], ["kind", "Kind"]] as const).map(([key, label]) => ({
      label: `Sort by ${label}${b.sort.key === key ? (b.sort.asc ? " ↑" : " ↓") : ""}`,
      checked: b.sort.key === key,
      run: () => b.sortBy(key),
    })),
    "separator",
    { label: b.showHidden ? "Hide Hidden Files" : "Show Hidden Files", shortcut: shortcutLabel("Mod+H"), run: () => b.toggleHidden() },
    ...(b.git && b.view === "list" ? [{ label: "Show Git Column", checked: prefs.gitColumn, run: () => (prefs.gitColumn = !prefs.gitColumn) }] : []),
    ...(b.view === "icons"
      ? ["separator" as const,
        ...ICON_SIZES.map((s): MenuItem => ({ label: `${s.label} Icons`, checked: prefs.iconSize === s.size, run: () => (prefs.iconSize = s.size) })),
        { label: "Crop Thumbnails to Squares", checked: prefs.thumbFit === "fill", run: () => (prefs.thumbFit = prefs.thumbFit === "fill" ? "fit" : "fill") }]
      : []),
    "separator",
    { label: "Open in Terminal", disabled: !b.location, run: () => b.location && void openTerminalAt(b.location) },
    { label: "Copy Path", run: () => void copyPath(b) },
    { label: "Refresh", shortcut: shortcutLabel("Mod+R"), run: () => void b.refresh() },
  ];
  ui.openMenu(e, items);
}

// ---- terminal ----

/** The OS terminal in this folder; on a server, an ssh session there. */
export async function openTerminalAt(loc: Location) {
  try {
    if (loc.source.kind === "ssh") await hosts.connect(loc.source.host);
    await api.openTerminal(loc);
  } catch (e) {
    toasts.error(e, "Couldn't open a terminal");
  }
}

/** A terminal on a server, in its home folder. */
export async function openServerTerminal(alias: string) {
  const source: SourceId = { kind: "ssh", host: alias };
  try {
    await hosts.connect(alias);
    await api.openTerminal({ source, path: await api.homeDir(source) });
  } catch (e) {
    toasts.error(e, `Couldn't open a terminal on “${alias}”`);
  }
}

export function hostMenu(b: Browser, e: MouseEvent, alias: string) {
  const host = hosts.get(alias);
  if (!host) return;
  const state = host.status.state;
  const app = host.origin === "app";
  const items: MenuItem[] = [
    { label: "Open", run: () => void openHost(b, alias) },
    state === "connected" || state === "connecting"
      ? { label: "Disconnect", run: () => void disconnectHost(b, alias) }
      : { label: "Connect", run: () => void connectHost(alias) },
    { label: "Open in Terminal", run: () => void openServerTerminal(alias) },
    "separator",
    app
      ? { label: "Edit Server…", run: () => editServer(alias) }
      : { label: "Edit ~/.ssh/config", run: () => void editSshConfig() },
    { label: "Get Info", run: () => (ui.sheet = { kind: "host", host }) },
    { label: "Forget Saved Password", disabled: !host.hasSavedSecret, run: () => void forgetPassword(alias) },
    "separator",
    { label: "Copy Address", run: () => void copyText(hostAddress(host)) },
    { label: "Copy SSH Command", run: () => void copyText(sshCommand(host)) },
    ...(app ? ["separator" as const, { label: "Remove Server…", danger: true, run: () => void removeServer(alias) }] : []),
  ];
  ui.openMenu(e, items);
}

// ---- servers ----

export async function connectHost(alias: string) {
  try {
    await hosts.connect(alias);
  } catch (e) {
    toasts.error(e, `Couldn't connect to “${alias}”`, { label: "Retry", run: () => void connectHost(alias) });
  }
}

/** "user@host" plus ":port" when it isn't 22. */
export function hostAddress(h: HostInfo): string {
  return `${h.user ? `${h.user}@` : ""}${h.hostname}${h.port !== 22 ? `:${h.port}` : ""}`;
}

function sshCommand(h: HostInfo): string {
  // Config hosts resolve by alias; app hosts need the full target.
  if (h.origin === "sshConfig") return `ssh ${h.alias}`;
  const key = h.identityFiles[0] ? ` -i ${h.identityFiles[0]}` : "";
  return `ssh${h.port !== 22 ? ` -p ${h.port}` : ""}${key} ${h.user ? `${h.user}@` : ""}${h.hostname}`;
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    toasts.info("Couldn't access the system clipboard");
  }
}

export function addServer() {
  ui.sheet = { kind: "hostEdit", host: null };
}

export function editServer(alias: string) {
  const host = hosts.get(alias);
  if (host?.origin === "app") ui.sheet = { kind: "hostEdit", host };
}

/** Config hosts are edited in the user's own editor. */
export async function editSshConfig() {
  try {
    await api.openExternal({ source: local, path: await api.sshConfigPath() });
  } catch (e) {
    toasts.error(e, "Couldn't open ~/.ssh/config");
  }
}

export async function forgetPassword(alias: string) {
  try {
    await api.forgetPassword(alias);
    await hosts.reload().catch(() => {});
    toasts.info(`Forgot the saved password for “${alias}”`, "You'll be asked for it on the next connection.");
  } catch (e) {
    toasts.error(e, "Couldn't remove the saved password");
  }
}

export async function removeServer(alias: string) {
  const ok = await ui.confirm({
    title: `Remove “${alias}”?`,
    message: "The server and its saved password are removed from VPS Files. Nothing on the server is changed.",
    confirm: "Remove",
    danger: true,
  });
  if (!ok) return;
  try {
    await api.removeHost(alias);
  } catch (e) {
    toasts.error(e, `Couldn't remove “${alias}”`);
    return;
  }
  listings.dropSource({ kind: "ssh", host: alias });
  for (const t of tabs.all) {
    if (t.location?.source.kind === "ssh" && t.location.source.host === alias) openStart(t);
    t.backStack = t.backStack.filter((s) => !(s?.source.kind === "ssh" && s.source.host === alias));
    t.forwardStack = t.forwardStack.filter((s) => !(s?.source.kind === "ssh" && s.source.host === alias));
  }
  await hosts.reload().catch(() => {});
}

export async function disconnectHost(b: Browser, alias: string) {
  try {
    await hosts.disconnect(alias);
  } catch (e) {
    toasts.error(e, `Couldn't disconnect from “${alias}”`);
    return;
  }
  if (b.location?.source.kind === "ssh" && b.location.source.host === alias) {
    // Leave the server like Finder ejecting a volume.
    openStart(b);
  }
}

function pasteLabel(): string {
  if (clipboard.empty) return "Paste";
  const n = clipboard.items.length;
  const what = n === 1 ? "Item" : `${n} Items`;
  return clipboard.op === "move" ? `Move ${what} Here` : `Paste ${what}`;
}

function deleteShortcut(): string {
  return isMac ? shortcutLabel("Mod+Backspace") : "Delete";
}

/** True if `loc` is what `b` is showing. Used to refresh after transfers. */
export function isShowing(b: Browser, loc: Location): boolean {
  return sameLocation(b.location, loc);
}
