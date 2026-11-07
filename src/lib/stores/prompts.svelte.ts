// Backend-initiated questions (host key, password, passphrase), answered in
// a sheet. They queue: only the oldest is on screen.

import { api } from "$lib/ipc/api";
import type { PromptRequest } from "$lib/ipc/types";
import { hosts } from "./hosts.svelte";
import { toasts } from "./toasts.svelte";

class Prompts {
  queue = $state.raw<PromptRequest[]>([]);
  current = $derived(this.queue[0] ?? null);

  async init() {
    await api.on("prompt:request", (p) => {
      if (hosts.isQuiet(p.host)) {
        // Safety net: quiet connects are non-interactive, so this shouldn't
        // happen; if it does, never pop a sheet for a startup connect.
        hosts.declinedPrompt(p.host);
        void api.promptRespond(p.id, false, null).catch(() => {});
        return;
      }
      this.queue = [...this.queue, p];
    });
  }

  /** `remember` saves the password/passphrase in the OS keychain. */
  async respond(accept: boolean, secret: string | null = null, remember = false) {
    const p = this.current;
    if (!p) return;
    this.queue = this.queue.slice(1);
    try {
      await api.promptRespond(p.id, accept, accept ? secret : null, accept && remember);
    } catch (e) {
      toasts.error(e, "Couldn't send the answer");
    }
  }
}

export const prompts = new Prompts();
