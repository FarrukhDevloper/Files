// Lazy thumbnails for every view. With the media server (Tauri) images and
// videos come pre-scaled from `/thumb/{size}/…` (src/lib/ipc/media.ts); in
// a plain browser only small web images load, at full size, through
// api.mediaUrl. One shared IntersectionObserver; at most LOCAL_LOADS /
// REMOTE_LOADS loading at once (separate, so a slow server never holds up
// local files); a thumbnail that scrolls away before it finished gives its
// slot back and drops its src (which aborts the request).

import { api } from "$lib/ipc/api";
import { initMedia, thumbUrl } from "$lib/ipc/media";
import type { Entry, Location } from "$lib/ipc/types";
import { extension } from "$lib/util/filetype";

const LOCAL_LOADS = 8;
const REMOTE_LOADS = 6;
/** Row icons (list/column view) never ask for more than this × DPR. */
const ROW_MAX = 32;
const MB = 1024 * 1024;
/** Decoded by the backend (crates/stream). Its size limits; larger → 404. */
const SERVER_IMAGES = new Set(["jpg", "jpeg", "png", "webp", "gif", "bmp"]);
const SERVER_IMAGE_LIMIT = { local: 100 * MB, remote: 25 * MB };
/** One frame via ffmpeg (404 quickly when ffmpeg is missing). */
const SERVER_VIDEOS = new Set(["mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "flv", "mpg", "mpeg", "3gp"]);
/** Drawn by the webview itself from the full file: small ones only. */
const DIRECT = new Set(["jpg", "jpeg", "png", "webp", "gif", "avif", "svg"]);
const DIRECT_LIMIT = { local: 8 * MB, remote: 2 * MB };
/** Thumbnail pixel sizes the backend caches; requests round up to one. */
const BUCKETS = [32, 48, 64, 96, 128, 192, 256, 384, 512];

const media = $state({ ready: false });

/** Once at startup: learn the media server address. */
export function initThumbs(): Promise<void> {
  return initMedia().then((base) => { media.ready = !!base; }, () => {});
}

type Mode = "server" | "direct" | null;

function mode(entry: Entry, loc: Location): Mode {
  if (entry.kind !== "file") return null;
  const ext = extension(entry.name);
  const where = loc.source.kind === "local" ? "local" : "remote";
  if (media.ready) {
    if (SERVER_IMAGES.has(ext)) return entry.size <= SERVER_IMAGE_LIMIT[where] ? "server" : null;
    if (SERVER_VIDEOS.has(ext)) return "server";
  }
  return DIRECT.has(ext) && entry.size <= DIRECT_LIMIT[where] ? "direct" : null;
}

/** Whether to draw a thumbnail for `entry`. `small` = row icons (list and
 *  column view): only cheap pre-scaled server thumbnails, never full images. */
export function wantsThumbnail(entry: Entry, loc: Location, small = false): boolean {
  const m = mode(entry, loc);
  return small ? m === "server" : m !== null;
}

export function isVideoThumb(entry: Entry): boolean {
  return media.ready && SERVER_VIDEOS.has(extension(entry.name));
}

/** URL for a `size` CSS px box, or a promise of the full file's URL. */
function source(entry: Entry, loc: Location, size: number): string | Promise<string> | null {
  const m = mode(entry, loc);
  if (m === "server") {
    const dpr = globalThis.devicePixelRatio || 1;
    const want = Math.ceil(size * dpr);
    let px = BUCKETS.find((b) => b >= want) ?? 512;
    if (size <= ROW_MAX) px = Math.min(px, Math.max(32, Math.floor(ROW_MAX * dpr)));
    return thumbUrl(loc, px, entry.modified);
  }
  return m === "direct" ? api.mediaUrl(loc) : null;
}

/** Keys whose image failed once; they stay as type icons. */
const failed = new Set<string>();
export function hasFailed(key: string): boolean {
  return failed.has(key);
}

// ---- concurrency gates: one for local files, one for servers ----

type Job = { run: () => void };

class Gate {
  #queue: Job[] = [];
  #loading = 0;
  constructor(readonly max: number) {}

  enqueue(job: Job) {
    this.#queue.push(job);
    this.#pump();
  }

  dequeue(job: Job) {
    const i = this.#queue.indexOf(job);
    if (i >= 0) this.#queue.splice(i, 1);
  }

  release() {
    this.#loading = Math.max(0, this.#loading - 1);
    this.#pump();
  }

  #pump() {
    while (this.#loading < this.max && this.#queue.length) {
      this.#loading++;
      this.#queue.shift()!.run();
    }
  }
}

const gates = { local: new Gate(LOCAL_LOADS), remote: new Gate(REMOTE_LOADS) };

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
export function watchThumbnail(
  el: Element, entry: Entry, loc: Location, size: number, key: string, show: (url: string | null) => void,
): ThumbHandle {
  let state: "idle" | "queued" | "loading" | "done" = "idle";
  const gate = loc.source.kind === "local" ? gates.local : gates.remote;

  const fail = () => {
    failed.add(key);
    if (state === "loading") {
      state = "done";
      gate.release();
    }
  };

  const job: Job = {
    run: () => {
      state = "loading";
      const src = source(entry, loc, size);
      if (src === null) fail();
      else if (typeof src === "string") show(src);
      else src.then((url) => { if (state === "loading") show(url); }, fail);
    },
  };

  observe(el, (visible) => {
    if (visible && state === "idle") {
      state = "queued";
      gate.enqueue(job);
    } else if (!visible && state === "queued") {
      gate.dequeue(job);
      state = "idle";
    } else if (!visible && state === "loading") {
      show(null);
      state = "idle";
      gate.release();
    }
  });

  return {
    settled(ok) {
      if (!ok) failed.add(key);
      if (state === "loading") {
        state = "done";
        gate.release();
      }
    },
    dispose() {
      unobserve(el);
      if (state === "queued") gate.dequeue(job);
      if (state === "loading") gate.release();
      state = "done";
    },
  };
}
