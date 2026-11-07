// Frameless-window controls. Inside Tauri they drive the native window; in
// a plain browser (`pnpm dev`) they are no-ops.

import { isTauri } from "$lib/ipc/api";

type AppWindow = import("@tauri-apps/api/window").Window;

let current: Promise<AppWindow> | null = null;

function win(): Promise<AppWindow> | null {
  if (!isTauri) return null;
  current ??= import("@tauri-apps/api/window").then((m) => m.getCurrentWindow());
  return current;
}

export const appWindow = {
  async close() { await (await win())?.close(); },
  async minimize() { await (await win())?.minimize(); },
  async toggleMaximize() { await (await win())?.toggleMaximize(); },
};

export type FileDragEvent =
  | { type: "enter"; paths: string[]; position: { x: number; y: number } }
  | { type: "over"; position: { x: number; y: number } }
  | { type: "drop"; paths: string[]; position: { x: number; y: number } }
  | { type: "leave" };

/** Files dragged in from the OS (Tauri only). Positions are physical pixels. */
export async function onFileDrag(handler: (e: FileDragEvent) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((e) => handler(e.payload as FileDragEvent));
}
