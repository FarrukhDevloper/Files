// One browser = one tab: location, history, listing, sort, view mode,
// selection. Views read `items` (filtered + sorted) and call the selection
// methods; file operations live in $lib/actions.

import { api } from "$lib/ipc/api";
import type { Entry, Location, VfsError } from "$lib/ipc/types";
import { toVfsError } from "$lib/util/errors";
import { kindLabel } from "$lib/util/filetype";
import { ancestors, baseName, isRoot, parentPath, sameLocation, sameSource, sourceLabel } from "$lib/util/path";
import { hosts } from "./hosts.svelte";
import { prefs, type Sort, type SortKey, type ViewMode } from "./prefs.svelte";
import { toasts } from "./toasts.svelte";

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** Folders first, then by `sort`; ties (e.g. folder sizes) by name A→Z. */
export function sortEntries(list: readonly Entry[], sort: Sort): Entry[] {
  const byName = (a: Entry, b: Entry) => collator.compare(a.name, b.name);
  let cmp: (a: Entry, b: Entry) => number = byName;
  if (sort.key === "modified") cmp = (a, b) => (a.modified ?? 0) - (b.modified ?? 0);
  else if (sort.key === "size") cmp = (a, b) => a.size - b.size;
  else if (sort.key === "kind") {
    const kinds = new Map(list.map((e) => [e, kindLabel(e)]));
    cmp = (a, b) => collator.compare(kinds.get(a)!, kinds.get(b)!);
  }
  const dir = sort.asc ? 1 : -1;
  return [...list].sort(
    (a, b) => (a.kind === "dir" ? 0 : 1) - (b.kind === "dir" ? 0 : 1) || dir * cmp(a, b) || byName(a, b),
  );
}

export interface GoOptions {
  /** "push" (default) records history; "none" is for back/forward/refresh. */
  history?: "push" | "none";
  /** Select and reveal this path after loading. */
  select?: string | null;
}

export class Browser {
  location = $state.raw<Location | null>(null);
  entries = $state.raw<Entry[]>([]);
  loading = $state(false);
  /** Set when the current folder couldn't be listed (shown in place of the list). */
  error = $state.raw<VfsError | null>(null);
  backStack = $state.raw<Location[]>([]);
  forwardStack = $state.raw<Location[]>([]);

  view = $state<ViewMode>(prefs.view);
  sort = $state.raw<Sort>(prefs.sort);
  showHidden = $state(prefs.showHidden);
  filter = $state("");

  selected = $state.raw<ReadonlySet<string>>(new Set());
  /** Keyboard focus item (last clicked / arrowed). */
  cursor = $state<string | null>(null);
  /** Path being renamed inline. */
  renaming = $state<string | null>(null);
  /** Changes whenever the listing is for a different folder; views reset scroll. */
  listingId = $state(0);
  /** Bumped when the same folder is listed again (refresh); caches drop stale data. */
  refreshCount = $state(0);
  /** Ask views to scroll a path into view; `seq` re-triggers for the same path. */
  reveal = $state.raw<{ path: string; seq: number } | null>(null);
  /** Columns in the icon grid, written by IconView, used by arrow keys. */
  gridColumns = 1;
  /** Scroll position saved by the view, restored when switching back to this tab. */
  scrollMemory: { view: ViewMode; listingId: number; top: number } | null = null;

  items = $derived.by(() => {
    const q = this.filter.trim().toLocaleLowerCase();
    const shown = this.entries.filter(
      (e) => (this.showHidden || !e.isHidden) && (!q || e.name.toLocaleLowerCase().includes(q)),
    );
    return sortEntries(shown, this.sort);
  });
  index = $derived.by(() => {
    const m = new Map<string, number>();
    this.items.forEach((e, i) => m.set(e.path, i));
    return m;
  });
  selection = $derived(this.items.filter((e) => this.selected.has(e.path)));
  title = $derived(
    this.location ? (isRoot(this.location.path) ? sourceLabel(this.location.source) : baseName(this.location.path)) : "VPS Files",
  );

  #anchor: string | null = null;
  #seq = 0;
  #revealSeq = 0;
  #typed = "";
  #typedAt = 0;

  get canBack() { return this.backStack.length > 0; }
  get canForward() { return this.forwardStack.length > 0; }
  get canUp() { return !!this.location && parentPath(this.location.path) !== null; }

  /** List `loc` and show it. Resolves false if it failed or was superseded. */
  async go(loc: Location, opts: GoOptions = {}): Promise<boolean> {
    const seq = ++this.#seq;
    const from = this.location;
    const same = sameLocation(loc, from);
    this.loading = true;
    try {
      const list = await this.#list(loc, seq);
      if (list === null) return false;

      if (!same && from && (opts.history ?? "push") === "push") {
        this.backStack = [...this.backStack, from].slice(-100);
        this.forwardStack = [];
      }
      this.location = loc;
      this.entries = list;
      this.error = null;

      if (same) {
        this.refreshCount++;
        const alive = new Set(list.map((e) => e.path));
        this.selected = new Set([...this.selected].filter((p) => alive.has(p)));
        if (this.cursor && !alive.has(this.cursor)) this.cursor = null;
        if (this.renaming && !alive.has(this.renaming)) this.renaming = null;
        if (opts.select) this.selectOnly(opts.select);
      } else {
        this.listingId++;
        this.filter = "";
        this.renaming = null;
        // Going up: select the folder we came out of, like Finder.
        let select = opts.select;
        if (select === undefined && from && sameSource(from.source, loc.source)) {
          const chain = ancestors(from.path);
          const at = chain.indexOf(loc.path);
          select = at >= 0 ? chain[at + 1] ?? null : null;
        }
        this.selectOnly(select ?? null);
      }
      return true;
    } catch (e) {
      if (seq !== this.#seq) return false;
      const label = baseName(loc.path) || sourceLabel(loc.source);
      if (!from) {
        // Nothing to stay on: show the error in place of the listing.
        this.location = loc;
        this.entries = [];
        this.error = toVfsError(e);
        this.listingId++;
      } else {
        toasts.error(e, `Couldn't open “${label}”`);
      }
      return false;
    } finally {
      if (seq === this.#seq) this.loading = false;
    }
  }

  /** listDir, connecting first if the server dropped. null = superseded. */
  async #list(loc: Location, seq: number): Promise<Entry[] | null> {
    try {
      const list = await api.listDir(loc);
      return seq === this.#seq ? list : null;
    } catch (e) {
      if (loc.source.kind !== "ssh" || toVfsError(e).code !== "not_connected") throw e;
      await hosts.connect(loc.source.host);
      if (seq !== this.#seq) return null;
      const list = await api.listDir(loc);
      return seq === this.#seq ? list : null;
    }
  }

  async back() {
    const target = this.backStack.at(-1);
    const from = this.location;
    if (!target || !from) return;
    if (await this.go(target, { history: "none" })) {
      this.backStack = this.backStack.slice(0, -1);
      this.forwardStack = [...this.forwardStack, from];
    }
  }

  async forward() {
    const target = this.forwardStack.at(-1);
    const from = this.location;
    if (!target || !from) return;
    if (await this.go(target, { history: "none" })) {
      this.forwardStack = this.forwardStack.slice(0, -1);
      this.backStack = [...this.backStack, from];
    }
  }

  async up() {
    const parent = this.location && parentPath(this.location.path);
    if (parent !== null && this.location) await this.go({ source: this.location.source, path: parent });
  }

  async refresh() {
    if (this.location) await this.go(this.location, { history: "none" });
  }

  /** Disconnected/failed state: forget the listing but keep the place. */
  invalidate(error: VfsError) {
    this.#seq++;
    this.loading = false;
    this.entries = [];
    this.error = error;
    this.selected = new Set();
    this.cursor = null;
  }

  // ---- view settings (per tab; defaults come from the Preferences sheet) ----

  setView(view: ViewMode) {
    this.view = view;
  }

  sortBy(key: SortKey) {
    const asc = this.sort.key === key ? !this.sort.asc : key === "name" || key === "kind";
    this.sort = { key, asc };
    prefs.sort = this.sort;
    if (this.cursor) this.revealPath(this.cursor);
  }

  toggleHidden() {
    this.setHidden(!this.showHidden);
  }

  setHidden(show: boolean) {
    this.showHidden = show;
    const visible = new Set(this.items.map((e) => e.path));
    this.selected = new Set([...this.selected].filter((p) => visible.has(p)));
  }

  // ---- selection ----

  selectOnly(path: string | null) {
    this.selected = path ? new Set([path]) : new Set();
    this.cursor = path;
    this.#anchor = path;
    if (path) this.revealPath(path);
  }

  selectNone() {
    this.selected = new Set();
    this.#anchor = null;
  }

  selectAll() {
    this.selected = new Set(this.items.map((e) => e.path));
  }

  /** Mouse selection: plain = only, toggle = Ctrl/⌘, range = Shift. */
  click(path: string, mods: { toggle: boolean; range: boolean }) {
    if (mods.range && this.#anchor && this.index.has(this.#anchor)) {
      const range = this.#range(this.#anchor, path);
      this.selected = mods.toggle ? new Set([...this.selected, ...range]) : new Set(range);
      this.cursor = path;
    } else if (mods.toggle) {
      const next = new Set(this.selected);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      this.selected = next;
      this.cursor = path;
      this.#anchor = path;
    } else {
      this.selectOnly(path);
    }
  }

  /** Arrow keys. `delta` in items; `extend` = Shift. */
  moveCursor(delta: number, extend = false) {
    const n = this.items.length;
    if (n === 0) return;
    const at = this.cursor !== null ? this.index.get(this.cursor) : undefined;
    let next: number;
    if (at === undefined) next = delta > 0 ? 0 : n - 1;
    else next = Math.max(0, Math.min(n - 1, at + delta));
    this.moveTo(next, extend);
  }

  moveTo(index: number, extend = false) {
    const entry = this.items[Math.max(0, Math.min(this.items.length - 1, index))];
    if (!entry) return;
    if (extend && this.#anchor && this.index.has(this.#anchor)) {
      this.selected = new Set(this.#range(this.#anchor, entry.path));
      this.cursor = entry.path;
      this.revealPath(entry.path);
    } else {
      this.selectOnly(entry.path);
    }
  }

  /** Type-to-select: letters typed within a second form a prefix. */
  typeSelect(ch: string) {
    const now = performance.now();
    this.#typed = now - this.#typedAt > 1000 ? ch : this.#typed + ch;
    this.#typedAt = now;
    const prefix = this.#typed.toLocaleLowerCase();
    // Exact prefix first; otherwise the alphabetically closest name after it.
    let hit = this.items.find((e) => e.name.toLocaleLowerCase().startsWith(prefix));
    if (!hit) {
      for (const e of this.items) {
        if (collator.compare(e.name, prefix) >= 0 && (!hit || collator.compare(e.name, hit.name) < 0)) hit = e;
      }
    }
    if (hit) this.selectOnly(hit.path);
  }

  revealPath(path: string) {
    this.reveal = { path, seq: ++this.#revealSeq };
  }

  #range(a: string, b: string): string[] {
    const i = this.index.get(a) ?? 0;
    const j = this.index.get(b) ?? 0;
    return this.items.slice(Math.min(i, j), Math.max(i, j) + 1).map((e) => e.path);
  }

  // ---- local edits after a successful command (no re-list round-trip) ----

  addEntry(entry: Entry) {
    this.entries = [...this.entries.filter((e) => e.path !== entry.path), entry];
  }

  replaceEntry(oldPath: string, entry: Entry) {
    this.entries = this.entries.map((e) => (e.path === oldPath ? entry : e));
    if (this.selected.has(oldPath)) {
      const next = new Set(this.selected);
      next.delete(oldPath);
      next.add(entry.path);
      this.selected = next;
    }
    if (this.cursor === oldPath) this.cursor = entry.path;
  }

  removePaths(paths: readonly string[]) {
    const gone = new Set(paths);
    const firstIndex = Math.min(...paths.map((p) => this.index.get(p) ?? Infinity));
    this.entries = this.entries.filter((e) => !gone.has(e.path));
    this.selectNone();
    this.cursor = null;
    if (Number.isFinite(firstIndex) && this.items.length > 0) this.moveTo(Math.min(firstIndex, this.items.length - 1));
  }

  locationOf(entry: Entry): Location {
    return { source: this.location!.source, path: entry.path };
  }
}
