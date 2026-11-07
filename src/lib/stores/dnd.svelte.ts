// Internal drag & drop of files, built on mouse events rather than HTML5
// DnD: Tauri's OS file-drop handling disables HTML5 DnD inside the webview
// on some platforms. Drop targets are plain elements carrying
//   data-drop="dir"  data-drop-source="<sourceKey>" data-drop-path="/abs"
//   data-drop="host" data-drop-host="<alias>"
// and are found by hit-testing under the pointer.

import type { Location } from "$lib/ipc/types";
import type { IconType } from "$lib/util/filetype";
import { ancestors, parentPath, parseSourceKey, sameSource, sourceKey } from "$lib/util/path";

export type DropTarget =
  | { kind: "dir"; key: string; location: Location }
  | { kind: "host"; key: string; host: string };

export interface DragPayload {
  items: Location[];
  label: string;
  icon: IconType;
}

type DropHandler = (target: DropTarget, items: Location[], copy: boolean) => void;

const THRESHOLD = 4;
const EDGE = 28;

/** Key a drop target element highlights itself with. */
export function dropKey(loc: Location): string {
  return `${sourceKey(loc.source)}|${loc.path}`;
}

class Dnd {
  active = $state(false);
  x = $state(0);
  y = $state(0);
  payload = $state.raw<DragPayload | null>(null);
  target = $state.raw<DropTarget | null>(null);
  /** True when dropping would copy (cross-source, or Ctrl/Alt held). */
  copy = $state(false);

  /** Files are being dragged in from the OS (see $lib/osDrop). */
  external = $state(false);
  /** External drag over the file view with no folder under the pointer. */
  externalOnView = $state(false);

  onDrop: DropHandler = () => {};

  #start = { x: 0, y: 0 };
  #begin: (() => DragPayload | null) | null = null;
  #suppressClick = false;

  /** mousedown on an item: start watching for a drag. */
  arm(e: MouseEvent, begin: () => DragPayload | null) {
    if (e.button !== 0) return;
    this.#start = { x: e.clientX, y: e.clientY };
    this.#begin = begin;
    window.addEventListener("mousemove", this.#move, true);
    window.addEventListener("mouseup", this.#up, true);
    window.addEventListener("keydown", this.#key, true);
    window.addEventListener("blur", this.#cancel);
  }

  /** The click that ends a drag must not also select/rename. */
  consumeClick(): boolean {
    const s = this.#suppressClick;
    this.#suppressClick = false;
    return s;
  }

  #move = (e: MouseEvent) => {
    if (!this.active) {
      if (Math.hypot(e.clientX - this.#start.x, e.clientY - this.#start.y) < THRESHOLD) return;
      const payload = this.#begin?.() ?? null;
      if (!payload || payload.items.length === 0) return this.#cancel();
      this.payload = payload;
      this.active = true;
      document.documentElement.classList.add("dragging");
    }
    e.preventDefault();
    this.x = e.clientX;
    this.y = e.clientY;
    this.#hitTest(e);
    autoScroll(e);
  };

  #up = (e: MouseEvent) => {
    if (this.active) {
      e.preventDefault();
      this.#suppressClick = true;
      setTimeout(() => (this.#suppressClick = false), 0);
      const { target, payload, copy } = this;
      this.#cancel();
      if (target && payload) this.onDrop(target, payload.items, copy);
    } else {
      this.#cancel();
    }
  };

  #key = (e: KeyboardEvent) => {
    if (!this.active) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      this.#cancel();
    } else if (e.key === "Control" || e.key === "Alt" || e.key === "Meta") {
      this.copy = this.#wantsCopy(e);
    }
  };

  #cancel = () => {
    window.removeEventListener("mousemove", this.#move, true);
    window.removeEventListener("mouseup", this.#up, true);
    window.removeEventListener("keydown", this.#key, true);
    window.removeEventListener("blur", this.#cancel);
    document.documentElement.classList.remove("dragging", "drag-copy");
    this.active = false;
    this.payload = null;
    this.target = null;
    this.#begin = null;
  };

  /** OS drag at CSS point (x, y): any folder/server target, else the file view. */
  externalAt(x: number, y: number) {
    this.external = true;
    const hit = document.elementFromPoint(x, y) as HTMLElement | null;
    const el = hit?.closest<HTMLElement>("[data-drop]");
    this.target = el ? readTarget(el) : null;
    this.externalOnView = !this.target && !!hit?.closest(".content");
  }

  externalEnd() {
    this.external = false;
    this.externalOnView = false;
    this.target = null;
  }

  #hitTest(e: MouseEvent) {
    const el = (document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null)?.closest<HTMLElement>("[data-drop]");
    const target = el ? readTarget(el) : null;
    this.target = target && this.#accepts(target) ? target : null;
    this.copy = this.#wantsCopy(e);
    document.documentElement.classList.toggle("drag-copy", !!this.target && this.copy);
  }

  #wantsCopy(e: MouseEvent | KeyboardEvent): boolean {
    const items = this.payload?.items ?? [];
    const t = this.target;
    if (!t || items.length === 0) return false;
    const cross = t.kind === "host"
      ? items[0]!.source.kind !== "ssh" || items[0]!.source.host !== t.host
      : !sameSource(items[0]!.source, t.location.source);
    return cross || e.ctrlKey || e.altKey;
  }

  /** No drops onto a dragged folder, into its own subtree, or back into the same folder. */
  #accepts(t: DropTarget): boolean {
    const items = this.payload?.items ?? [];
    if (t.kind === "host") return true;
    const dest = t.location;
    for (const it of items) {
      if (sameSource(it.source, dest.source) && ancestors(dest.path).includes(it.path)) return false;
    }
    return !items.every((it) => sameSource(it.source, dest.source) && parentPath(it.path) === dest.path);
  }
}

function readTarget(el: HTMLElement): DropTarget | null {
  const d = el.dataset;
  if (d.drop === "host" && d.dropHost) return { kind: "host", key: `host:${d.dropHost}`, host: d.dropHost };
  if (d.drop === "dir" && d.dropPath && d.dropSource) {
    const location = { source: parseSourceKey(d.dropSource), path: d.dropPath };
    return { kind: "dir", key: dropKey(location), location };
  }
  return null;
}

/** Nudge a scrolling list when the pointer nears its top/bottom edge. */
function autoScroll(e: MouseEvent) {
  const vp = (document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null)?.closest<HTMLElement>(".viewport, .column-scroll");
  if (!vp) return;
  const r = vp.getBoundingClientRect();
  if (e.clientY < r.top + EDGE) vp.scrollTop -= 24;
  else if (e.clientY > r.bottom - EDGE) vp.scrollTop += 24;
}

export const dnd = new Dnd();
