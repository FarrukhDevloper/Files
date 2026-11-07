// Per-tab scroll memory for the virtualized views: a view remounts when the
// tab changes, restores where that tab was, and resets to the top when the
// tab lists a different folder.

import type { Browser } from "$lib/stores/browser.svelte";
import type { ViewMode } from "$lib/stores/prefs.svelte";

export function scrollMemory(b: () => Browser, view: ViewMode) {
  let seen = -1;
  return {
    /** Call from an effect that depends on `b().listingId`. Returns the scrollTop to apply. */
    onListing(listingId: number): number {
      const first = seen === -1;
      seen = listingId;
      const m = b().scrollMemory;
      return first && m && m.view === view && m.listingId === listingId ? m.top : 0;
    },
    save(top: number) {
      b().scrollMemory = { view, listingId: seen, top };
    },
  };
}
