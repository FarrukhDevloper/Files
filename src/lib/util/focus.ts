// Focus helpers for modal surfaces (sheets, Quick Look, menus, popovers).

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function tabbables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    // Only the checked radio of a group is in the Tab order.
    (el) => el.getClientRects().length > 0 && !(el instanceof HTMLInputElement && el.type === "radio" && !el.checked),
  );
}

/** Keep Tab / Shift+Tab inside `root`. Call from a keydown handler. */
export function trapTab(e: KeyboardEvent, root: HTMLElement | undefined) {
  if (e.key !== "Tab" || !root) return;
  const items = tabbables(root);
  const active = document.activeElement;
  if (items.length === 0) {
    e.preventDefault();
    root.focus();
    return;
  }
  const first = items[0]!;
  const last = items.at(-1)!;
  if (e.shiftKey && (active === first || !root.contains(active))) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && (active === last || !root.contains(active))) {
    e.preventDefault();
    first.focus();
  }
}

/** Remember what has focus now; the returned function puts it back. */
export function rememberFocus(): () => void {
  const el = document.activeElement as HTMLElement | null;
  return () => {
    if (el && el.isConnected && el !== document.body) el.focus({ preventScroll: true });
  };
}
