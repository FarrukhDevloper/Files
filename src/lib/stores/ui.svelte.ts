// Transient UI surfaces: context menu, sheets, Quick Look, popovers.

import type { Entry, HostInfo, Location } from "$lib/ipc/types";

export type MenuItem =
  | { label: string; shortcut?: string; disabled?: boolean; danger?: boolean; checked?: boolean; run: () => void }
  | "separator";

export interface Menu { x: number; y: number; items: MenuItem[]; keyboard: boolean }

export interface ConfirmOptions {
  title: string;
  message: string;
  confirm: string;
  danger?: boolean;
}

export type ConflictAction = "stop" | "skip" | "keepBoth" | "replace" | "merge";

export interface ConflictOptions {
  name: string;
  /** Display name of the destination folder. */
  folder: string;
  op: "copy" | "move";
  /** Folder↔file clash: only Keep Both / Skip make sense. */
  allowReplace: boolean;
  /** Folder onto folder: copying into it is possible. */
  allowMerge: boolean;
  existing: Entry;
  incoming: Entry | null;
  /** Conflicts still to decide, including this one. */
  remaining: number;
}

export type Sheet =
  | ({ kind: "confirm"; resolve: (ok: boolean) => void } & ConfirmOptions)
  | ({ kind: "conflict"; resolve: (r: { action: ConflictAction; all: boolean }) => void } & ConflictOptions)
  | { kind: "prefs" }
  | { kind: "goto"; location: Location }
  | { kind: "info"; entry: Entry; location: Location }
  | { kind: "host"; host: HostInfo };

class Ui {
  menu = $state.raw<Menu | null>(null);
  sheet = $state.raw<Sheet | null>(null);
  quickLook = $state(false);
  transfersOpen = $state(false);
  windowFocused = $state(true);
  /** Registered by the toolbar search field. */
  focusSearch: () => void = () => {};

  openMenu(e: MouseEvent, items: MenuItem[]) {
    e.preventDefault();
    e.stopPropagation();
    // Menu key / Shift+F10 on a focused control fires contextmenu at (0,0)
    // in some webviews: anchor to the element instead.
    let { clientX: x, clientY: y } = e;
    // Keyboard-initiated (Menu key, Shift+F10, ours with detail -1): not the right button.
    const keyboard = e.detail === -1 || e.button !== 2;
    if (x === 0 && y === 0 && e.currentTarget instanceof HTMLElement) {
      const r = e.currentTarget.getBoundingClientRect();
      x = r.left + 16;
      y = r.bottom;
    }
    this.menu = { x, y, items, keyboard };
  }

  /** Registered by Content: put keyboard focus back on the file view. */
  focusFiles: () => void = () => {};

  closeMenu() {
    this.menu = null;
  }

  confirm(opts: ConfirmOptions): Promise<boolean> {
    return new Promise((resolve) => {
      this.sheet = { kind: "confirm", ...opts, resolve };
    });
  }

  askConflict(opts: ConflictOptions): Promise<{ action: ConflictAction; all: boolean }> {
    return new Promise((resolve) => {
      this.sheet = { kind: "conflict", ...opts, resolve };
    });
  }

  closeSheet() {
    const s = this.sheet;
    this.sheet = null;
    if (s?.kind === "confirm") s.resolve(false);
    else if (s?.kind === "conflict") s.resolve({ action: "stop", all: false });
  }
}

export const ui = new Ui();
