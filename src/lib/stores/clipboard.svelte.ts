// In-app clipboard for Copy / Cut → Paste. Pasting starts a transfer, so it
// works across sources (local ↔ server ↔ server).

import type { Location, TransferOp } from "$lib/ipc/types";
import { sourceKey } from "$lib/util/path";

class Clipboard {
  op = $state<TransferOp>("copy");
  items = $state.raw<Location[]>([]);
  /** `sourceKey|path` of cut items, so views can dim them. */
  cutKeys = $derived(new Set(this.op === "move" ? this.items.map((l) => `${sourceKey(l.source)}|${l.path}`) : []));

  get empty() {
    return this.items.length === 0;
  }

  set(op: TransferOp, items: Location[]) {
    this.op = op;
    this.items = items;
  }

  clear() {
    this.items = [];
  }
}

export const clipboard = new Clipboard();
