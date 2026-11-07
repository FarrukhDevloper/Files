// Lazy image thumbnails for the icon view and the column info pane.
// One shared IntersectionObserver; at most MAX_LOADS images loading at once;
// an image that scrolls away before it finished loading gives its slot back
// and drops its src. Only small-enough images of web-safe types qualify.

import { api } from "$lib/ipc/api";
import type { Entry, Location } from "$lib/ipc/types";
import { extension } from "$lib/util/filetype";

const MAX_LOADS = 4;
const LOCAL_LIMIT = 8 * 1024 * 1024;
const REMOTE_LIMIT = 2 * 1024 * 1024;
const TYPES = new Set(["jpg", "jpeg", "png", "webp", "gif", "avif", "svg"]);

export function wantsThumbnail(entry: Entry, loc: Location): boolean {
  if (entry.kind !== "file" || !TYPES.has(extension(entry.name))) return false;
  return entry.size <= (loc.source.kind === "local" ? LOCAL_LIMIT : REMOTE_LIMIT);
}

/** Keys whose image failed once; they stay as type icons. */
const failed = new Set<string>();
export function hasFailed(key: string): boolean {
  return failed.has(key);
}

// ---- concurrency gate ----

type Job = { run: () => void };
const queue: Job[] = [];
let loading = 0;

function enqueue(job: Job) {
  queue.push(job);
  pump();
}

function dequeue(job: Job) {
  const i = queue.indexOf(job);
  if (i >= 0) queue.splice(i, 1);
}

function release() {
  loading = Math.max(0, loading - 1);
  pump();
}

function pump() {
  while (loading < MAX_LOADS && queue.length) {
    loading++;
    queue.shift()!.run();
  }
}

// ---- visibility ----

const watchers = new Map<Element, (visible: boolean) => void>();
let observer: IntersectionObserver | null = null;

function observe(el: Element, fn: (visible: boolean) => void) {
  observer ??= new IntersectionObserver(
    (records) => { for (const r of records) watchers.get(r.target)?.(r.isIntersecting); },
    { rootMargin: "64px" },
  );
  watchers.set(el, fn);
  observer.observe(el);
}

function unobserve(el: Element) {
  watchers.delete(el);
  observer?.unobserve(el);
}

// ---- per-thumbnail lifecycle ----

export interface ThumbHandle {
  /** The <img> finished (ok = loaded, false = error). */
  settled(ok: boolean): void;
  dispose(): void;
}

/** Watch `el`; when visible and a slot is free, resolve the URL and call
 *  `show(url)`; `show(null)` cancels an unfinished load. */
export function watchThumbnail(el: Element, loc: Location, key: string, show: (url: string | null) => void): ThumbHandle {
  let state: "idle" | "queued" | "loading" | "done" = "idle";

  const job: Job = {
    run: () => {
      state = "loading";
      api.mediaUrl(loc).then(
        (url) => { if (state === "loading") show(url); },
        () => {
          failed.add(key);
          if (state === "loading") { state = "done"; release(); }
        },
      );
    },
  };

  observe(el, (visible) => {
    if (visible && state === "idle") {
      state = "queued";
      enqueue(job);
    } else if (!visible && state === "queued") {
      dequeue(job);
      state = "idle";
    } else if (!visible && state === "loading") {
      show(null);
      state = "idle";
      release();
    }
  });

  return {
    settled(ok) {
      if (!ok) failed.add(key);
      if (state === "loading") {
        state = "done";
        release();
      }
    },
    dispose() {
      unobserve(el);
      if (state === "queued") dequeue(job);
      if (state === "loading") release();
      state = "done";
    },
  };
}
