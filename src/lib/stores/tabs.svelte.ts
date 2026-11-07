// Tabs: one Browser each. The strip shows only with two or more tabs
// (like Finder); everything else works on `tabs.current`.

import type { Location } from "$lib/ipc/types";
import { Browser } from "./browser.svelte";

class Tabs {
  list = $state.raw<Browser[]>([new Browser()]);
  active = $state(0);
  current = $derived(this.list[this.active] ?? this.list[0]!);

  /** New tab right after the active one, showing `at` (default: the same folder). */
  open(from: Browser = this.current, at: Location | null = from.location): Browser {
    const b = new Browser();
    b.view = from.view;
    b.sort = from.sort;
    b.showHidden = from.showHidden;
    const i = this.active + 1;
    this.list = [...this.list.slice(0, i), b, ...this.list.slice(i)];
    this.active = i;
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
    if (index >= 0 && index < this.list.length) this.active = index;
  }

  cycle(delta: 1 | -1) {
    const n = this.list.length;
    this.active = (this.active + delta + n) % n;
  }
}

export const tabs = new Tabs();
