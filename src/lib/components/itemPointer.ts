// Mouse behaviour shared by the list and icon views: Finder-style select on
// mousedown, collapse a multi-selection on click, click-to-rename on the name
// of an already selected item, double-click to open.

import { openEntry } from "$lib/actions";
import type { Entry } from "$lib/ipc/types";
import type { Browser } from "$lib/stores/browser.svelte";
import { dnd, type DragPayload } from "$lib/stores/dnd.svelte";
import { iconFor } from "$lib/util/filetype";
import { plural } from "$lib/util/format";
import { hasMod } from "$lib/util/platform";

const RENAME_DELAY = 650;

export function itemPointer(browser: () => Browser) {
  let renameTimer: ReturnType<typeof setTimeout> | undefined;
  let wasSoleSelected = false;

  return {
    down(e: MouseEvent, entry: Entry) {
      clearTimeout(renameTimer);
      if (e.button !== 0) return;
      const b = browser();
      if (b.renaming === entry.path) return;
      wasSoleSelected = b.selected.size === 1 && b.selected.has(entry.path);
      const toggle = hasMod(e);
      const range = e.shiftKey;
      if (toggle || range || !b.selected.has(entry.path)) b.click(entry.path, { toggle, range });
      else b.cursor = entry.path;
      if (!toggle && !range) dnd.arm(e, () => dragPayload(b));
    },

    click(e: MouseEvent, entry: Entry, onName: boolean) {
      const b = browser();
      if (dnd.consumeClick()) return;
      if (e.button !== 0 || hasMod(e) || e.shiftKey || b.renaming) return;
      if (b.selected.size > 1) {
        b.selectOnly(entry.path);
        return;
      }
      if (onName && wasSoleSelected && e.detail === 1) {
        renameTimer = setTimeout(() => {
          if (b.selected.size === 1 && b.selected.has(entry.path)) b.renaming = entry.path;
        }, RENAME_DELAY);
      }
    },

    dblclick(e: MouseEvent, entry: Entry) {
      clearTimeout(renameTimer);
      if (hasMod(e) || e.shiftKey || browser().renaming === entry.path) return;
      void openEntry(browser(), entry);
    },

    cancel() {
      clearTimeout(renameTimer);
    },
  };
}

function dragPayload(b: Browser): DragPayload | null {
  const sel = b.selection;
  if (!b.location || sel.length === 0 || b.renaming) return null;
  return {
    items: sel.map((e) => b.locationOf(e)),
    label: sel.length === 1 ? sel[0]!.name : plural(sel.length, "item"),
    icon: iconFor(sel[0]!),
  };
}
