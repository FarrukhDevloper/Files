// Tabs: one Browser each. The strip shows only with two or more tabs
// (like Finder). Dual pane (F3) adds a second, independent Browser on the
// right; `current` is whichever pane has focus, and everything else
// (toolbar, address bar, keyboard) works on it.

import type { Location } from "$lib/ipc/types";
import { Browser } from "./browser.svelte";

class Tabs {
  list = $state.raw<Browser[]>([new Browser()]);
  active = $state(0);
  /** The right pane in dual-pane mode. */
  split = $state.raw<Browser | null>(null);
  /** Focused pane: 0 = left (the tabs), 1 = right. */
  focus = $state<0 | 1>(0);
  /** The left pane: the active tab. */
  primary = $derived(this.list[this.active] ?? this.list[0]!);
  current = $derived(this.focus === 1 && this.split ? this.split : this.primary);
  /** The pane without focus (dual pane), for F5/F6. */
  other = $derived(this.split ? (this.focus === 1 ? this.primary : this.split) : null);

  /** Every Browser on screen or in a tab: refresh after transfers etc. */
  get all(): Browser[] {
    return this.split ? [...this.list, this.split] : this.list;
  }

  /** New tab right after the active one, showing `at` (null: nothing yet —
   *  the caller shows the start page, see actions.newTab). */
  open(from: Browser = this.current, at: Location | null = null): Browser {
    const b = new Browser();
    b.view = from.view;
    b.sort = from.sort;
    b.showHidden = from.showHidden;
    const i = this.active + 1;
    this.list = [...this.list.slice(0, i), b, ...this.list.slice(i)];
    this.active = i;
    this.focus = 0;
    if (at) void b.go(at, { history: "none" });
    return b;
  }

  /** The last tab never closes (that would close the window). */
  close(index = this.active) {
    if (this.list.length <= 1 || index < 0 || index >= this.list.length) return;
    this.list = this.list.filter((_, i) => i !== index);
    if (index < this.active || this.active >= this.list.length) this.active = Math.max(0, this.active - 1);
  }

  select(index: number) {
    if (index >= 0 && index < this.list.length) {
      this.active = index;
      this.focus = 0;
    }
  }

  cycle(delta: 1 | -1) {
    const n = this.list.length;
    this.active = (this.active + delta + n) % n;
    this.focus = 0;
  }

  /** F3: a second pane showing the same folder, or back to one pane. */
  toggleSplit(): Browser | null {
    if (this.split) {
      this.split = null;
      this.focus = 0;
      return null;
    }
    const from = this.primary;
    const b = new Browser();
    b.view = from.view;
    b.sort = from.sort;
    b.showHidden = from.showHidden;
    this.split = b;
    this.focus = 1;
    return b;
  }
}

export const tabs = new Tabs();
