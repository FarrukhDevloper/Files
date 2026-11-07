// User-level file operations shared by the keyboard, the context menu, the
// toolbar and the views. Each one reports failures as toasts.

import { api } from "$lib/ipc/api";
import type { Entry, Location, Place, SourceId, TransferOp } from "$lib/ipc/types";
import type { Browser } from "$lib/stores/browser.svelte";
import { clipboard } from "$lib/stores/clipboard.svelte";
import type { DropTarget } from "$lib/stores/dnd.svelte";
import { hosts } from "$lib/stores/hosts.svelte";
import { prefs } from "$lib/stores/prefs.svelte";
import { tabs } from "$lib/stores/tabs.svelte";
import { toasts } from "$lib/stores/toasts.svelte";
import { transfers } from "$lib/stores/transfers.svelte";
import { ui, type ConflictAction, type MenuItem } from "$lib/stores/ui.svelte";
import { previewKind } from "$lib/util/filetype";
import { plural } from "$lib/util/format";
import { ancestors, baseName, isRoot, joinPath, parentPath, sameLocation, sameSource, sourceLabel } from "$lib/util/path";
import { isMac, shortcutLabel } from "$lib/util/platform";

const local: SourceId = { kind: "local" };

// ---- navigation ----

export async function openPlace(b: Browser, place: Place) {
  await b.go({ source: local, path: place.path });
}

/** Sidebar server click: connect if needed, then open the remote home. */
export async function openHost(b: Browser, alias: string) {
  const source: SourceId = { kind: "ssh", host: alias };
  try {
    await hosts.connect(alias);
  } catch (e) {
    toasts.error(e, `Couldn't connect to “${alias}”`, { label: "Retry", run: () => void openHost(b, alias) });
    return;
  }
  try {
    const home = await api.homeDir(source);
    await b.go({ source, path: home });
  } catch (e) {
    toasts.error(e, `Couldn't open “${alias}”`);
  }
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

/** Enter / double-click. Folders navigate; local files open in the OS app;
 *  remote files open in Quick Look (stream, don't download). */
export async function openEntry(b: Browser, entry: Entry) {
  const loc = b.locationOf(entry);
  if (entry.kind === "dir") {
    await b.go(loc);
  } else if (loc.source.kind === "local") {
    await api.openExternal(loc).catch((e) => toasts.error(e, `Couldn't open “${entry.name}”`));
  } else if (previewKind(entry) !== "none") {
    b.selectOnly(entry.path);
    ui.quickLook = true;
  } else {
    toasts.info(`“${entry.name}” is on ${loc.source.host}`, "Copy it to this computer to open it with an app.");
  }
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

export async function commitRename(b: Browser, entry: Entry, newName: string) {
  b.renaming = null;
  const name = newName.trim();
  if (!name || name === entry.name) return;
  try {
    const renamed = await api.renamePath(b.locationOf(entry), name);
    b.replaceEntry(entry.path, renamed);
    b.selectOnly(renamed.path);
  } catch (e) {
    toasts.error(e, `Couldn't rename “${entry.name}”`);
  }
}

export async function newFolder(b: Browser) {
  if (!b.location || b.error) return;
  const taken = new Set(b.entries.map((e) => e.name.toLocaleLowerCase()));
  let name = "untitled folder";
  for (let n = 2; taken.has(name); n++) name = `untitled folder ${n}`;
  try {
    const entry = await api.createDir({ source: b.location.source, path: joinPath(b.location.path, name) });
    if (!b.showHidden && entry.isHidden) b.toggleHidden();
    b.filter = "";
    b.addEntry(entry);
    b.selectOnly(entry.path);
    b.renaming = entry.path;
  } catch (e) {
    toasts.error(e, "Couldn't create a folder");
  }
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
  const locs = sel.map((e) => b.locationOf(e));
  try {
    await api.deletePaths(locs, remote || permanent);
    b.removePaths(sel.map((e) => e.path));
  } catch (e) {
    toasts.error(e, `Couldn't delete ${what}`);
    await b.refresh();
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
    existing = new Map((await api.listDir(dest)).map((e) => [norm(e.name), e]));
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

  let started = false;
  if (groups.keepBoth.length) started = (await transfers.start({ op, sources: groups.keepBoth, destDir: dest, conflict: "keepBoth" })) !== null || started;
  if (groups.replace.length) started = (await transfers.start({ op, sources: groups.replace, destDir: dest, conflict: "overwrite" })) !== null || started;
  if (groups.merge.length) started = (await transfers.start({ op, sources: groups.merge, destDir: dest, conflict: "merge" })) !== null || started;
  return started;
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

export function goToFolder(b: Browser) {
  if (!b.location) return;
  ui.sheet = { kind: "goto", location: b.location };
}

/** Go-to-folder sheet: absolute path, "~" = home of the current source. */
export async function goToPath(b: Browser, source: SourceId, typed: string): Promise<boolean> {
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
  }
  if (!isRoot(path)) path = path.replace(/\/+$/, "") || "/";
  const loc: Location = { source, path };
  try {
    const entry = await api.statPath(loc);
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
    ...(dir ? [{ label: "Open in New Tab", run: () => void tabs.open(b, b.locationOf(dir)) }] : []),
    { label: "Quick Look", shortcut: "Space", disabled: !single, run: () => toggleQuickLook(b) },
    { label: "Get Info", shortcut: shortcutLabel("Mod+I"), disabled: !single, run: () => getInfo(b) },
    "separator",
    { label: "Rename", shortcut: "F2", disabled: !single, run: () => startRename(b) },
    { label: single ? "Copy Path" : "Copy Paths", run: () => void copyPath(b) },
    "separator",
    { label: "Copy", shortcut: shortcutLabel("Mod+C"), run: () => copySelection(b, "copy") },
    { label: "Cut", shortcut: shortcutLabel("Mod+X"), run: () => copySelection(b, "move") },
    { label: pasteLabel(), shortcut: shortcutLabel("Mod+V"), disabled: clipboard.empty, run: () => void paste(b) },
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
    { label: "Copy Path", run: () => void copyPath(b) },
    { label: "Refresh", shortcut: shortcutLabel("Mod+R"), run: () => void b.refresh() },
  ];
  ui.openMenu(e, items);
}

export function hostMenu(b: Browser, e: MouseEvent, alias: string) {
  const host = hosts.get(alias);
  if (!host) return;
  const state = host.status.state;
  const items: MenuItem[] = [
    { label: "Open", run: () => void openHost(b, alias) },
    state === "connected" || state === "connecting"
      ? { label: "Disconnect", run: () => void disconnectHost(b, alias) }
      : { label: "Connect", run: () => void hosts.connect(alias).catch((err) => toasts.error(err, `Couldn't connect to “${alias}”`)) },
    "separator",
    { label: "Get Info", run: () => (ui.sheet = { kind: "host", host }) },
    { label: "Copy SSH Command", run: () => void navigator.clipboard?.writeText(`ssh ${alias}`).catch(() => {}) },
    "separator",
    { label: "Reload ~/.ssh/config", run: () => void hosts.reload().catch((err) => toasts.error(err, "Couldn't read ~/.ssh/config")) },
  ];
  ui.openMenu(e, items);
}

export async function disconnectHost(b: Browser, alias: string) {
  try {
    await hosts.disconnect(alias);
  } catch (e) {
    toasts.error(e, `Couldn't disconnect from “${alias}”`);
    return;
  }
  if (b.location?.source.kind === "ssh" && b.location.source.host === alias) {
    // Leave the server like Finder ejecting a volume: fall back to local home.
    await openLocalHome(b);
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
