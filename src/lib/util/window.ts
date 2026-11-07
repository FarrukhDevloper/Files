// Frameless-window controls. Inside Tauri they drive the native window; in
// a plain browser (`pnpm dev`) they are no-ops.

import { isTauri } from "$lib/ipc/api";
import { zoomLevel } from "./zoom";

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

/** Files dragged in from the OS (Tauri only). Positions are converted from
 *  physical webview pixels to CSS px (display scale × interface zoom). */
export async function onFileDrag(handler: (e: FileDragEvent) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const [{ getCurrentWebview }, { getCurrentWindow }] = await Promise.all([
    import("@tauri-apps/api/webview"),
    import("@tauri-apps/api/window"),
  ]);
  const w = getCurrentWindow();
  let scale = await w.scaleFactor();
  const offScale = await w.onScaleChanged((e) => { scale = e.payload.scaleFactor; });
  const offDrop = await getCurrentWebview().onDragDropEvent(({ payload: p }) => {
    if (p.type === "leave") return handler(p);
    const k = scale * zoomLevel();
    handler({ ...p, position: { x: p.position.x / k, y: p.position.y / k } } as FileDragEvent);
  });
  return () => {
    offScale();
    offDrop();
  };
}

/** Native "open file" dialog (Tauri only); null if cancelled or in a browser. */
export async function pickFile(opts: { title?: string; defaultPath?: string }): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ ...opts, multiple: false, directory: false });
  return typeof picked === "string" ? picked : null;
}
