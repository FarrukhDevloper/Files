// Remote files opened in local apps (api.openExternal): a temp copy is
// downloaded, opened in the OS default app, and every save is uploaded back.
// Sessions update on "edit:update"; the toolbar lists them and a conflict
// (the server copy changed meanwhile) asks the user what to keep.

import { api } from "$lib/ipc/api";
import type { EditSession, Location } from "$lib/ipc/types";
import { baseName, parentPath, sameLocation, sourceLabel } from "$lib/util/path";
import { tabs } from "./tabs.svelte";
import { toasts } from "./toasts.svelte";
import { ui } from "./ui.svelte";

class Edits {
  /** Open sessions, newest first ("closed" ones are dropped). */
  list = $state.raw<EditSession[]>([]);
  /** External media players on this machine, e.g. ["mpv", "vlc"]. */
  players = $state.raw<string[]>([]);
  busy = $derived(this.list.some((s) => s.state === "downloading" || s.state === "uploading"));
  attention = $derived(this.list.some((s) => s.state === "conflict" || s.state === "failed"));
  open = $state(false);

  async init() {
    await api.on("edit:update", (s) => this.#update(s, true));
    const [list, players] = await Promise.all([api.editList(), api.externalPlayers().catch(() => [])]);
    for (const s of list) this.#update(s, false);
    this.players = players;
  }

  /** Remote file: download, open in its default app, sync saves back. */
  async start(loc: Location) {
    const s = await api.openExternal(loc);
    if (s) this.#update(s, false);
  }

  async stop(id: string) {
    try {
      await api.editStop(id);
      this.list = this.list.filter((s) => s.id !== id);
    } catch (e) {
      toasts.error(e, "Couldn't stop syncing");
    }
  }

  async resolve(id: string, action: "upload" | "discard") {
    try {
      await api.editResolve(id, action);
    } catch (e) {
      toasts.error(e, action === "upload" ? "Couldn't upload your copy" : "Couldn't reload the server copy");
    }
  }

  /** Conflict sheet, or a toast to come back to if a sheet is already up. */
  askConflict(s: EditSession) {
    if (!ui.sheet) {
      ui.sheet = { kind: "editConflict", session: s };
      return;
    }
    toasts.push({
      tone: "error",
      title: `“${baseName(s.location.path)}” changed on ${sourceLabel(s.location.source)}`,
      message: "Both your copy and the server copy changed.",
      action: { label: "Resolve…", run: () => this.askConflict(this.list.find((x) => x.id === s.id) ?? s) },
    });
  }

  #update(s: EditSession, notify: boolean) {
    const prev = this.list.find((x) => x.id === s.id);
    if (s.state === "closed") this.list = this.list.filter((x) => x.id !== s.id);
    else this.list = prev ? this.list.map((x) => (x.id === s.id ? s : x)) : [s, ...this.list];
    if (notify && prev?.state !== s.state) this.#announce(prev, s);
  }

  #announce(prev: EditSession | undefined, s: EditSession) {
    const name = baseName(s.location.path);
    const where = sourceLabel(s.location.source);
    if (s.state === "synced" && prev?.state === "uploading") {
      toasts.info(`Saved “${name}” to ${where}`);
      // Show the new size and date wherever the folder is open.
      const dir = { source: s.location.source, path: parentPath(s.location.path) ?? "/" };
      for (const b of tabs.all) if (sameLocation(b.location, dir)) b.revalidate();
    } else if (s.state === "failed") {
      toasts.error(s.error ?? { code: "other", message: "Sync failed" }, `Couldn't sync “${name}”`);
    } else if (s.state === "conflict") {
      this.askConflict(s);
    }
  }
}

export const edits = new Edits();
