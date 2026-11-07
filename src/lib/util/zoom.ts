// Interface size (Preferences, Ctrl+= / Ctrl+- / Ctrl+0).
// Inside Tauri the webview zooms natively, so every coordinate stays in CSS
// px. A plain browser falls back to CSS `zoom` on <html>; there pointer
// coordinates and getBoundingClientRect() are viewport px, so divide by
// cssZoom() before using one as a CSS length.

import { isTauri } from "$lib/ipc/api";

export const SIZES = [
  { scale: 0.9, label: "Small" },
  { scale: 1, label: "Default" },
  { scale: 1.15, label: "Large" },
  { scale: 1.3, label: "Larger" },
] as const;

let level = 1;
let webview: Promise<import("@tauri-apps/api/webview").Webview> | null = null;

/** Current zoom factor of the whole UI. */
export function zoomLevel(): number {
  return level;
}

/** Factor between viewport px and CSS px (1 inside Tauri). */
export function cssZoom(): number {
  return isTauri ? 1 : level;
}

export async function applyZoom(scale: number): Promise<void> {
  level = scale;
  if (isTauri) {
    webview ??= import("@tauri-apps/api/webview").then((m) => m.getCurrentWebview());
    await (await webview).setZoom(scale);
  } else {
    document.documentElement.style.zoom = scale === 1 ? "" : String(scale);
  }
}

/** Next size up (+1) or down (-1) from `scale`, clamped to the list. */
export function stepSize(scale: number, dir: 1 | -1): number {
  let i = SIZES.findIndex((s) => s.scale >= scale - 0.001);
  if (i < 0) i = SIZES.length - 1;
  return SIZES[Math.max(0, Math.min(SIZES.length - 1, i + dir))]!.scale;
}
