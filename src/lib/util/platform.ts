// Platform helpers: the primary modifier is ⌘ on macOS, Ctrl elsewhere.

export const isMac =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

export function hasMod(e: KeyboardEvent | MouseEvent): boolean {
  return isMac ? e.metaKey : e.ctrlKey;
}

/** "Mod+Shift+N" → "⇧⌘N" on macOS, "Ctrl+Shift+N" elsewhere. */
export function shortcutLabel(combo: string): string {
  const parts = combo.split("+");
  if (!isMac) return parts.map((p) => (p === "Mod" ? "Ctrl" : p)).join("+");
  const glyph: Record<string, string> = { Mod: "⌘", Shift: "⇧", Alt: "⌥", Ctrl: "⌃", Backspace: "⌫", Delete: "⌦", Enter: "↩", Up: "↑", Down: "↓", Left: "←", Right: "→" };
  const order = ["Ctrl", "Alt", "Shift", "Mod"];
  const mods = parts.slice(0, -1).sort((a, b) => order.indexOf(a) - order.indexOf(b));
  const key = parts.at(-1) ?? "";
  return mods.map((m) => glyph[m] ?? m).join("") + (glyph[key] ?? key);
}

/** True when keyboard input belongs to a text field, not the file view. */
export function isTextInput(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  return el.isContentEditable || el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT";
}
