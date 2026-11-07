// Listing cache shared by every tab and the column view: the last MAX
// folders listed. A cached folder opens at once and is refreshed in the
// background (stale-while-revalidate, see Browser.go). Prefetch fills it
// when the pointer rests on a folder or the keyboard selects one; requests
// for the same folder share one listDir.

import { api } from "$lib/ipc/api";
import type { Entry, Location, SourceId } from "$lib/ipc/types";
import { sourceKey } from "$lib/util/path";
import { hosts } from "./hosts.svelte";

const MAX = 300;
const PREFETCH_DELAY = 150;

const cache = new Map<string, Entry[]>();
const inflight = new Map<string, Promise<Entry[]>>();

function keyOf(loc: Location): string {
  return `${sourceKey(loc.source)}|${loc.path}`;
}

function store(key: string, list: Entry[]) {
  cache.delete(key);
  cache.set(key, list);
  if (cache.size > MAX) cache.delete(cache.keys().next().value!);
}

let hoverKey = "";
let hoverTimer: ReturnType<typeof setTimeout> | undefined;

export const listings = {
  /** Cached entries (marks them recently used), or undefined. */
  get(loc: Location): Entry[] | undefined {
    const key = keyOf(loc);
    const list = cache.get(key);
    if (list) store(key, list);
    return list;
  },

  /** listDir; concurrent calls for one folder share the request. */
  fetch(loc: Location): Promise<Entry[]> {
    const key = keyOf(loc);
    let p = inflight.get(key);
    if (!p) {
      p = api.listDir(loc).then((list) => {
        store(key, list);
        return list;
      });
      p.finally(() => inflight.delete(key)).catch(() => {});
      inflight.set(key, p);
    }
    return p;
  },

  /** Keep the cached copy in step with a local edit (rename, new, delete). */
  update(loc: Location, fn: (list: Entry[]) => Entry[]) {
    const key = keyOf(loc);
    const list = cache.get(key);
    if (list) cache.set(key, fn(list));
  },

  drop(loc: Location) {
    cache.delete(keyOf(loc));
  },

  /** Forget a whole server (removed or renamed). */
  dropSource(source: SourceId) {
    const prefix = `${sourceKey(source)}|`;
    for (const key of [...cache.keys()]) if (key.startsWith(prefix)) cache.delete(key);
  },

  /** Fill the cache if the folder isn't there yet (and its server is up). */
  prefetch(loc: Location) {
    const key = keyOf(loc);
    if (cache.has(key) || inflight.has(key)) return;
    if (loc.source.kind === "ssh" && !hosts.isConnected(loc.source.host)) return;
    listings.fetch(loc).catch(() => {});
  },

  /** Prefetch once `loc` stays hovered/selected for a moment; null cancels. */
  prefetchSoon(loc: Location | null) {
    const key = loc ? keyOf(loc) : "";
    if (key === hoverKey) return;
    hoverKey = key;
    clearTimeout(hoverTimer);
    if (loc) hoverTimer = setTimeout(() => listings.prefetch(loc), PREFETCH_DELAY);
  },
};

/** Equal for display purposes (same items, sizes, dates): skip the update. */
export function sameListing(a: readonly Entry[], b: readonly Entry[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  const byPath = new Map(a.map((e) => [e.path, e]));
  return b.every((e) => {
    const o = byPath.get(e.path);
    return !!o && o.size === e.size && o.modified === e.modified && o.kind === e.kind
      && o.mode === e.mode && o.isSymlink === e.isSymlink;
  });
}
