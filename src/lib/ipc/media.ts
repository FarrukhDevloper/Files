// Synchronous media + thumbnail URLs. Must match the Rust decoder in
// crates/stream: `/{token}/{source}/{seg}/…` and
// `/{token}/thumb/{size}/{source}/{seg}/…`, every segment percent-encoded.

import { api } from "./api";
import type { Location, SourceId } from "./types";

let base: string | null = null;
let pending: Promise<string> | null = null;

/** Resolve once at startup (or lazily); later calls are free. */
export function initMedia(): Promise<string> {
  pending ??= api.mediaBase().then((b) => (base = b));
  return pending;
}

function sourceStr(s: SourceId): string {
  return s.kind === "local" ? "local" : `ssh:${s.host}`;
}

function encodedPath(loc: Location): string {
  const segs = loc.path.split("/").filter(Boolean).map(encodeURIComponent);
  return `${encodeURIComponent(sourceStr(loc.source))}/${segs.join("/")}`;
}

/** Same URL as api.mediaUrl, or null before initMedia() resolved. */
export function mediaUrlSync(loc: Location): string | null {
  return base ? `${base}/${encodedPath(loc)}` : null;
}

/** Square-bounded thumbnail (JPEG/PNG). `modified` busts the HTTP cache. */
export function thumbUrl(loc: Location, size: number, modified: number | null): string | null {
  return base ? `${base}/thumb/${size}/${encodedPath(loc)}${modified ? `?v=${modified}` : ""}` : null;
}
