// One browser = one tab: location, history, listing, sort, view mode,
// selection. Views read `items` (filtered + sorted) and call the selection
// methods; file operations live in $lib/actions.

import { api } from "$lib/ipc/api";
import type { Entry, GitInfo, Location, VfsError } from "$lib/ipc/types";
import { toVfsError } from "$lib/util/errors";
import { kindLabel } from "$lib/util/filetype";
import { ancestors, baseName, isRoot, parentPath, sameLocation, sameSource, sourceKey, sourceLabel } from "$lib/util/path";
import { hosts } from "./hosts.svelte";
import { listings, sameListing } from "./listings";
import { ms } from "$lib/util/motion";
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

/** A history entry: a folder, or null for the Home start page. */
export type Stop = Location | null;

/** Last git state per folder, so a revisit shows its badges at once. */
const gitCache = new Map<string, GitInfo | null>();
const GIT_CACHE_MAX = 200;

interface ShowContext { here: Stop | undefined; from: Location | null; opts: GoOptions }

/** Into a subfolder = 1, out to an ancestor = -1, anything else = 0. */
function direction(from: Location | null, to: Location): -1 | 0 | 1 {
  if (!from || !sameSource(from.source, to.source)) return 0;
  if (ancestors(to.path).includes(from.path)) return 1;
  if (ancestors(from.path).includes(to.path)) return -1;
  return 0;
}

export class Browser {
  /** null while the Home start page is shown (or before the first listing). */
  location = $state.raw<Location | null>(null);
  /** Showing the Home start page (servers, drives) instead of a folder. */
  home = $state(false);
  entries = $state.raw<Entry[]>([]);
  /** Waiting for a folder that isn't cached yet (the old one stays shown). */
  loading = $state(false);
  /** Re-listing the shown folder in the background. */
  refreshing = $state(false);
  /** Items still being copied/moved here (dimmed until the job is done). */
  pending = $state.raw<ReadonlySet<string>>(new Set());
  /** Just added (rows animate in) / about to go (rows animate out). */
  fresh = $state.raw<ReadonlySet<string>>(new Set());
  leaving = $state.raw<ReadonlySet<string>>(new Set());
  /** Git state of the shown folder; null outside a repository (or unknown). */
  git = $state.raw<GitInfo | null>(null);
  /** Direction of the last navigation, for the view transition:
   *  1 = deeper/forward, -1 = up/back, 0 = elsewhere. */
  navDir: -1 | 0 | 1 = 0;
  /** Set when the current folder couldn't be listed (shown in place of the list). */
  error = $state.raw<VfsError | null>(null);
  backStack = $state.raw<Stop[]>([]);
  forwardStack = $state.raw<Stop[]>([]);

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
    this.home ? "Home"
      : this.location ? (isRoot(this.location.path) ? sourceLabel(this.location.source) : baseName(this.location.path))
      : "VPS Files",
  );

  #anchor: string | null = null;
  #seq = 0;
  #dirHint: -1 | 1 | null = null;
  #gitSeq = 0;
  #freshTimer: ReturnType<typeof setTimeout> | undefined;
  /** Navigation generation, bumped by every navigation the user starts. */
  #nav = 0;
  #revealSeq = 0;
  #typed = "";
  #typedAt = 0;

  get canBack() { return this.backStack.length > 0; }
  get canForward() { return this.forwardStack.length > 0; }
  get canUp() { return !!this.location && parentPath(this.location.path) !== null; }
  /** Where we are, as a history entry; undefined before anything was shown. */
  get #here(): Stop | undefined { return this.home ? null : this.location ?? undefined; }

  /** Claim the next navigation before a slow step (connecting, stat). Pass
   *  the token to isCurrentNav() afterwards: if the user went anywhere else
   *  in the meantime, the late result must not pull them back. */
  claimNav(): number {
    // A listing still in flight is superseded too.
    this.#seq++;
    this.loading = false;
    return ++this.#nav;
  }

  isCurrentNav(token: number): boolean {
    return token === this.#nav;
  }

  /** Show the Home start page. */
  showHome(opts: Pick<GoOptions, "history"> = {}) {
    this.#seq++;
    this.#nav++;
    const here = this.#here;
    if (!this.home && here !== undefined && (opts.history ?? "push") === "push") {
      this.backStack = [...this.backStack, here].slice(-100);
      this.forwardStack = [];
    }
    this.navDir = this.#dirHint ?? 0;
    this.#dirHint = null;
    this.#gitSeq++;
    this.git = null;
    this.home = true;
    this.location = null;
    this.entries = [];
    this.error = null;
    this.loading = false;
    this.filter = "";
    this.renaming = null;
    this.listingId++;
    this.selectOnly(null);
  }

  /** Go to a history entry. */
  #visit(stop: Stop): Promise<boolean> {
    if (stop === null) {
      this.showHome({ history: "none" });
      return Promise.resolve(true);
    }
    return this.go(stop, { history: "none" });
  }

  /** Show `loc`. A cached listing shows at once and is refreshed in the
   *  background; otherwise the current folder stays until the new one is
   *  listed. Resolves false if it failed or was superseded. */
  async go(loc: Location, opts: GoOptions = {}): Promise<boolean> {
    const seq = ++this.#seq;
    this.#nav++;
    const ctx: ShowContext = { here: this.#here, from: this.location, opts };
    const same = sameLocation(loc, ctx.from);
    if (!same) {
      const cached = listings.get(loc);
      if (cached) {
        this.loading = false;
        this.#show(loc, cached, ctx);
        void this.#revalidate(loc, seq);
        return true;
      }
    }
    // Re-listing the shown folder doesn't block it.
    if (same) this.refreshing = true;
    else this.loading = true;
    try {
      const list = await this.#list(loc, seq);
      if (list === null) return false;
      if (same) this.#merge(list, { force: true, select: opts.select });
      else this.#show(loc, list, ctx);
      return true;
    } catch (e) {
      if (seq !== this.#seq) return false;
      const label = baseName(loc.path) || sourceLabel(loc.source);
      if (!ctx.from && !this.home) {
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
      if (seq === this.#seq) {
        this.loading = false;
        this.refreshing = false;
      }
    }
  }

  /** Display a new folder's listing (history, selection, view reset). */
  #show(loc: Location, list: Entry[], { here, from, opts }: ShowContext) {
    if (here !== undefined && (opts.history ?? "push") === "push") {
      this.backStack = [...this.backStack, here].slice(-100);
      this.forwardStack = [];
    }
    this.navDir = this.#dirHint ?? direction(from, loc);
    this.#dirHint = null;
    this.home = false;
    this.location = loc;
    this.entries = list;
    this.pending = new Set();
    this.fresh = new Set();
    this.leaving = new Set();
    this.error = null;
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
    this.#loadGit(loc);
  }

  /** Git status for the shown folder, alongside its listing (never blocks it). */
  #loadGit(loc: Location) {
    const seq = ++this.#gitSeq;
    const key = `${sourceKey(loc.source)}|${loc.path}`;
    this.git = gitCache.get(key) ?? null;
    api.gitInfo(loc).then(
      (info) => {
        gitCache.delete(key);
        gitCache.set(key, info);
        if (gitCache.size > GIT_CACHE_MAX) gitCache.delete(gitCache.keys().next().value!);
        if (seq === this.#gitSeq) this.git = info;
      },
      () => { if (seq === this.#gitSeq) this.git = null; },
    );
  }

  /** A fresh listing of the shown folder: apply it in place (rows keyed by
   *  path, scroll and selection kept). Unchanged listings change nothing. */
  #merge(list: Entry[], { force = false, select }: { force?: boolean; select?: string | null } = {}) {
    if (sameListing(this.entries, list) && !this.pending.size) {
      if (force) {
        this.refreshCount++;
        if (this.location) this.#loadGit(this.location);
      }
    } else {
      if (this.location) this.#loadGit(this.location);
      const before = new Set(this.entries.map((e) => e.path));
      this.#markFresh(list.filter((e) => !before.has(e.path)).map((e) => e.path));
      this.entries = list;
      this.pending = new Set();
      this.refreshCount++;
      const alive = new Set(list.map((e) => e.path));
      if ([...this.selected].some((p) => !alive.has(p))) this.selected = new Set([...this.selected].filter((p) => alive.has(p)));
      if (this.cursor && !alive.has(this.cursor)) this.cursor = null;
      if (this.renaming && !alive.has(this.renaming)) this.renaming = null;
    }
    this.error = null;
    if (select) this.selectOnly(select);
  }

  /** Background refresh after showing a cached listing. */
  async #revalidate(loc: Location, seq: number) {
    this.refreshing = true;
    try {
      const list = await this.#list(loc, seq);
      if (list !== null) this.#merge(list);
    } catch (e) {
      if (seq !== this.#seq) return;
      const err = toVfsError(e);
      if (err.code === "not_found" || err.code === "not_a_directory" || err.code === "permission_denied") {
        // The folder went away: say so instead of showing old contents.
        listings.drop(loc);
        this.entries = [];
        this.error = err;
        this.listingId++;
      }
      // Offline or slow: keep showing the cached listing.
    } finally {
      if (seq === this.#seq) this.refreshing = false;
    }
  }

  /** listDir, connecting first if the server dropped. null = superseded. */
  async #list(loc: Location, seq: number): Promise<Entry[] | null> {
    try {
      const list = await listings.fetch(loc);
      return seq === this.#seq ? list : null;
    } catch (e) {
      if (loc.source.kind !== "ssh" || toVfsError(e).code !== "not_connected") throw e;
      await hosts.connect(loc.source.host);
      if (seq !== this.#seq) return null;
      const list = await listings.fetch(loc);
      return seq === this.#seq ? list : null;
    }
  }

  async back() {
    const target = this.backStack.at(-1);
    const from = this.#here;
    if (target === undefined || from === undefined) return;
    this.#dirHint = -1;
    if (await this.#visit(target)) {
      this.backStack = this.backStack.slice(0, -1);
      this.forwardStack = [...this.forwardStack, from];
    }
  }

  async forward() {
    const target = this.forwardStack.at(-1);
    const from = this.#here;
    if (target === undefined || from === undefined) return;
    this.#dirHint = 1;
    if (await this.#visit(target)) {
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

  /** The shown folder's listing changed elsewhere (transfer, reconnect):
   *  re-list it quietly; nothing happens if it's the same. */
  revalidate() {
    if (this.location && !this.loading) void this.#revalidate(this.location, this.#seq);
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
    // Keyboard on a folder: list it ahead, Enter will likely follow.
    if (entry.kind === "dir" && this.location) listings.prefetchSoon(this.locationOf(entry));
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

  /** Apply a local edit to folder `dir`: in this view if it shows it, and
   *  in the listing cache (so a revisit isn't stale). */
  #edit(dir: Location | null, fn: (list: Entry[]) => Entry[]) {
    if (!dir) return;
    if (sameLocation(this.location, dir)) this.entries = fn(this.entries);
    listings.update(dir, fn);
  }

  addEntry(entry: Entry, dir = this.location) {
    this.#edit(dir, (list) => [...list.filter((e) => e.path !== entry.path), entry]);
    if (sameLocation(this.location, dir)) this.#markFresh([entry.path]);
  }

  /** Rows for these paths play their insert animation once. */
  #markFresh(paths: string[]) {
    if (!paths.length || paths.length > 50) return; // a big change is a new listing, not an insert
    this.fresh = new Set([...this.fresh, ...paths]);
    clearTimeout(this.#freshTimer);
    this.#freshTimer = setTimeout(() => (this.fresh = new Set()), 400);
  }

  /** Items arriving from a transfer: shown dimmed until it settles. */
  addPending(entries: Entry[], dir: Location) {
    if (!sameLocation(this.location, dir)) return;
    const fresh = entries.filter((e) => !this.index.has(e.path));
    if (!fresh.length) return;
    this.entries = [...this.entries, ...fresh];
    this.pending = new Set([...this.pending, ...fresh.map((e) => e.path)]);
    this.#markFresh(fresh.map((e) => e.path));
  }

  replaceEntry(oldPath: string, entry: Entry, dir = this.location) {
    this.#edit(dir, (list) => list.map((e) => (e.path === oldPath ? entry : e)));
    if (!sameLocation(this.location, dir)) return;
    if (this.selected.has(oldPath)) {
      const next = new Set(this.selected);
      next.delete(oldPath);
      next.add(entry.path);
      this.selected = next;
    }
    if (this.cursor === oldPath) this.cursor = entry.path;
    if (this.renaming === oldPath) this.renaming = entry.path;
  }

  /** Remove rows; shown ones fade out first (`animate`, ~140 ms). */
  removePaths(paths: readonly string[], dir = this.location, animate = false) {
    const delay = animate && sameLocation(this.location, dir) ? ms(140) : 0;
    if (delay) {
      this.leaving = new Set([...this.leaving, ...paths]);
      setTimeout(() => {
        const left = new Set(paths);
        this.leaving = new Set([...this.leaving].filter((p) => !left.has(p)));
        this.removePaths(paths, dir);
      }, delay);
      return;
    }
    const gone = new Set(paths);
    const showing = sameLocation(this.location, dir);
    const firstIndex = showing ? Math.min(...paths.map((p) => this.index.get(p) ?? Infinity)) : Infinity;
    this.#edit(dir, (list) => list.filter((e) => !gone.has(e.path)));
    if (!showing) return;
    this.selectNone();
    this.cursor = null;
    if (Number.isFinite(firstIndex) && this.items.length > 0) this.moveTo(Math.min(firstIndex, this.items.length - 1));
  }

  locationOf(entry: Entry): Location {
    return { source: this.location!.source, path: entry.path };
  }
}
