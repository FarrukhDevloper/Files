// Global shortcuts (docs/DESIGN.md "Interaction"). Ctrl on Windows/Linux,
// ⌘ on macOS. Sheets, menus and text fields handle their own keys first.

import {
  backgroundMenu, copySelection, deleteSelection, entryMenu, getInfo, goToFolder, newFolder, openPreferences, openSelection,
  paste, startRename, toggleQuickLook,
} from "$lib/actions";
import { prompts } from "$lib/stores/prompts.svelte";
import { tabs } from "$lib/stores/tabs.svelte";
import { ui } from "$lib/stores/ui.svelte";
import { hasMod, isTextInput } from "$lib/util/platform";

export function handleKeydown(e: KeyboardEvent) {
  if (e.defaultPrevented || e.isComposing) return;
  if (ui.sheet || prompts.current || ui.menu) return;
  const mod = hasMod(e);
  const key = e.key;
  if (!ui.quickLook && tabKeys(e, mod)) {
    e.preventDefault();
    return;
  }
  if (isTextInput(e.target)) return;

  const b = tabs.current;
  // Plain keys (arrows, Enter, Space, letters, Delete…) belong to the file
  // view; on a focused button/tab/link they keep their native meaning.
  const plainAllowed = inFileView(e.target);
  let handled = true;

  if (ui.quickLook && key === "Escape") {
    e.preventDefault();
    ui.quickLook = false;
    return;
  }
  if (ui.quickLook && !inQuickLookControl(e.target)) {
    if (key === " " || key === "Escape") ui.quickLook = false;
    else if (key === "ArrowLeft" || key === "ArrowUp") b.moveCursor(key === "ArrowUp" && b.view === "icons" ? -b.gridColumns : -1);
    else if (key === "ArrowRight" || key === "ArrowDown") b.moveCursor(key === "ArrowDown" && b.view === "icons" ? b.gridColumns : 1);
    else if (key === "Enter" || (mod && key === "ArrowDown")) { ui.quickLook = false; void openSelection(b); }
    else handled = false;
    if (handled) e.preventDefault();
    return;
  }

  if (mod && !e.altKey) {
    switch (key.length === 1 ? key.toLowerCase() : key) {
      case "1": b.setView("icons"); break;
      case "2": b.setView("list"); break;
      case "3": b.setView("columns"); break;
      case ",": openPreferences(); break;
      case "a": b.selectAll(); break;
      case "c": copySelection(b, "copy"); break;
      case "x": copySelection(b, "move"); break;
      case "v": void paste(b); break;
      case "n": if (e.shiftKey) void newFolder(b); else handled = false; break;
      case "l": goToFolder(b); break;
      case "g": if (e.shiftKey) goToFolder(b); else handled = false; break;
      case "h": case ".": b.toggleHidden(); break;
      case "f": ui.focusSearch(); break;
      case "i": getInfo(b); break;
      case "r": void b.refresh(); break;
      case "y": toggleQuickLook(b); break;
      case "[": void b.back(); break;
      case "]": void b.forward(); break;
      case "ArrowUp": void b.up(); break;
      case "ArrowDown": void openSelection(b); break;
      case "Backspace": void deleteSelection(b); break;
      default: handled = false;
    }
  } else if (e.altKey && !mod && (key === "ArrowLeft" || key === "ArrowRight")) {
    void (key === "ArrowLeft" ? b.back() : b.forward());
  } else if (!mod && !e.altKey && plainAllowed) {
    const grid = b.view === "icons";
    if (key === "ContextMenu" || (e.shiftKey && key === "F10")) {
      e.preventDefault();
      keyboardMenu();
      return;
    }
    if (b.view === "columns" && (key === "ArrowLeft" || key === "ArrowRight") && !e.shiftKey) {
      e.preventDefault();
      void columnStep(key === "ArrowRight");
      return;
    }
    switch (key) {
      case "ArrowDown": b.moveCursor(grid ? b.gridColumns : 1, e.shiftKey); break;
      case "ArrowUp": b.moveCursor(grid ? -b.gridColumns : -1, e.shiftKey); break;
      case "ArrowRight": if (grid) b.moveCursor(1, e.shiftKey); else handled = false; break;
      case "ArrowLeft": if (grid) b.moveCursor(-1, e.shiftKey); else handled = false; break;
      case "Home": b.moveTo(0, e.shiftKey); break;
      case "End": b.moveTo(b.items.length - 1, e.shiftKey); break;
      case "PageDown": b.moveCursor(pageSize(), e.shiftKey); break;
      case "PageUp": b.moveCursor(-pageSize(), e.shiftKey); break;
      case "Enter": void openSelection(b); break;
      case "F2": startRename(b); break;
      case "F5": void b.refresh(); break;
      case " ": toggleQuickLook(b); break;
      case "Backspace": void b.up(); break;
      case "Delete": void deleteSelection(b, e.shiftKey); break;
      case "Escape":
        if (b.filter) b.filter = "";
        else b.selectNone();
        break;
      default:
        if (key.length === 1 && !e.ctrlKey && !e.metaKey && key !== " ") b.typeSelect(key);
        else handled = false;
    }
  } else {
    handled = false;
  }
  if (handled) e.preventDefault();
}

/** Focus is on the page itself or inside the file view, not on a control. */
function inFileView(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement) || t === document.body || t === document.documentElement) return true;
  return !!t.closest(".content") && !t.closest("button, a, input, select, textarea, [role=tab]");
}

function inQuickLookControl(t: EventTarget | null): boolean {
  return t instanceof HTMLElement && !!t.closest("[role=dialog] button");
}

/** Shift+F10 / Menu key: the context menu of the focused item, anchored to its row. */
function keyboardMenu() {
  const b = tabs.current;
  const path = b.cursor !== null && b.selected.has(b.cursor) ? b.cursor : b.selection[0]?.path ?? null;
  const entry = path !== null ? b.items[b.index.get(path) ?? -1] : undefined;
  const row = path !== null ? document.querySelector<HTMLElement>(`[data-path="${CSS.escape(path)}"]`) : null;
  const r = (row ?? document.querySelector(".content"))?.getBoundingClientRect();
  if (!r) return;
  // detail -1 marks a keyboard-opened menu (first item gets highlighted).
  const at = new MouseEvent("contextmenu", { clientX: r.left + 24, clientY: row ? r.bottom : r.top + 24, detail: -1 });
  if (entry) entryMenu(b, at, entry);
  else backgroundMenu(b, at);
}

/** Column view: → enters the selected folder (selecting its first item),
 *  ← goes back to the parent column (selecting the folder we left). */
async function columnStep(right: boolean) {
  const b = tabs.current;
  if (!right) {
    if (b.canUp) await b.up();
    return;
  }
  const sel = b.selection;
  if (sel.length !== 1 || sel[0]!.kind !== "dir") return;
  if (await b.go(b.locationOf(sel[0]!), { history: "none" })) b.moveTo(0);
}

/** Tab shortcuts work everywhere, even in the search field. */
function tabKeys(e: KeyboardEvent, mod: boolean): boolean {
  if (e.ctrlKey && e.key === "Tab") {
    tabs.cycle(e.shiftKey ? -1 : 1);
  } else if (e.ctrlKey && !e.shiftKey && !e.altKey && (e.key === "PageDown" || e.key === "PageUp")) {
    tabs.cycle(e.key === "PageDown" ? 1 : -1);
  } else if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "t") {
    tabs.open();
  } else if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "w") {
    // Never close the window from here; with one tab this is a no-op.
    tabs.close();
  } else {
    return false;
  }
  return true;
}

function pageSize(): number {
  return Math.max(1, Math.floor((window.innerHeight - 140) / 24));
}

/** Mouse buttons 4/5 = back/forward. */
export function handleMouseNav(e: MouseEvent) {
  if (e.button !== 3 && e.button !== 4) return;
  e.preventDefault();
  if (ui.sheet || prompts.current) return;
  const b = tabs.current;
  void (e.button === 3 ? b.back() : b.forward());
}
