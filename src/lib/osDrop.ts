// Files dropped from the OS file manager: copy them into the folder under
// the pointer (row, sidebar place, server, breadcrumb, tab) or else into the
// folder the active tab shows.

import { dropItems } from "$lib/actions";
import type { Location } from "$lib/ipc/types";
import { dnd, dropKey, type DropTarget } from "$lib/stores/dnd.svelte";
import { prompts } from "$lib/stores/prompts.svelte";
import { tabs } from "$lib/stores/tabs.svelte";
import { toasts } from "$lib/stores/toasts.svelte";
import { ui } from "$lib/stores/ui.svelte";
import { isRoot, parentPath, sameSource } from "$lib/util/path";
import { onFileDrag, type FileDragEvent } from "$lib/util/window";

/** OS path → VFS path (crates/vfs/src/path.rs): "/"-separated, Windows as
 *  "C:/Users/me". UNC shares aren't addressable yet → null. */
export function nativeToVfs(native: string): string | null {
  let p = native.replaceAll("\\", "/");
  if (p.startsWith("//?/") && /^[A-Za-z]:\//.test(p.slice(4))) p = p.slice(4); // \\?\C:\…
  if (p.startsWith("//")) return null;
  if (/^[A-Za-z]:$/.test(p)) p += "/";
  if (!p.startsWith("/") && !/^[A-Za-z]:\//.test(p)) return null;
  if (!isRoot(p)) p = p.replace(/\/+$/, "");
  return p;
}

function blocked(): boolean {
  return !!ui.sheet || !!prompts.current;
}

export function handleFileDrag(e: FileDragEvent) {
  if (e.type === "leave") {
    dnd.externalEnd();
    return;
  }
  if (blocked()) return;
  const { x, y } = e.position;
  if (e.type !== "drop") {
    dnd.externalAt(x, y);
    return;
  }

  dnd.externalAt(x, y);
  const b = tabs.current;
  let target: DropTarget | null = dnd.target;
  if (!target && dnd.externalOnView && b.location && !b.error) {
    target = { kind: "dir", key: dropKey(b.location), location: b.location };
  }
  dnd.externalEnd();
  if (!target) return;

  const items: Location[] = [];
  let skipped = 0;
  for (const native of e.paths) {
    const path = nativeToVfs(native);
    if (path) items.push({ source: { kind: "local" }, path });
    else skipped++;
  }
  if (skipped) toasts.info(`${skipped} item${skipped === 1 ? "" : "s"} skipped`, "Network (UNC) paths aren't supported yet.");
  if (items.length === 0) return;

  // Dropped back into the folder they came from: nothing to do (Finder).
  if (target.kind === "dir") {
    const dest = target.location;
    if (items.every((it) => sameSource(it.source, dest.source) && parentPath(it.path) === dest.path)) return;
  }
  void dropItems(target, items, true);
}

/** Subscribe once at startup; no-op outside Tauri. */
export function listenForFileDrops(): Promise<() => void> {
  return onFileDrag(handleFileDrag);
}
