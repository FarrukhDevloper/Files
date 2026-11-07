// Backend-initiated questions (host key, password, passphrase), answered in
// a sheet. They queue: only the oldest is on screen.

import { api } from "$lib/ipc/api";
import type { PromptRequest } from "$lib/ipc/types";
import { toasts } from "./toasts.svelte";

class Prompts {
  queue = $state.raw<PromptRequest[]>([]);
  current = $derived(this.queue[0] ?? null);

  async init() {
    await api.on("prompt:request", (p) => {
      this.queue = [...this.queue, p];
    });
  }

  async respond(accept: boolean, secret: string | null = null) {
    const p = this.current;
    if (!p) return;
    this.queue = this.queue.slice(1);
    try {
      await api.promptRespond(p.id, accept, accept ? secret : null);
    } catch (e) {
      toasts.error(e, "Couldn't send the answer");
    }
  }
}

export const prompts = new Prompts();
