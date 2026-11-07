// SSH hosts from ~/.ssh/config, their live connection status, and the local
// places shown in the sidebar.

import { api } from "$lib/ipc/api";
import type { ConnStatus, HostInfo, Place } from "$lib/ipc/types";
import { toVfsError } from "$lib/util/errors";

type StatusListener = (host: string, status: ConnStatus, prev: ConnStatus | undefined) => void;

/** "tanla via bastion" → "tanla". */
function promptAlias(host: string): string {
  const i = host.indexOf(" via ");
  return i < 0 ? host : host.slice(0, i);
}

class Hosts {
  list = $state.raw<HostInfo[]>([]);
  places = $state.raw<Place[]>([]);
  favourites = $derived(this.places.filter((p) => p.kind !== "root" && p.kind !== "drive"));
  locations = $derived(this.places.filter((p) => p.kind === "root" || p.kind === "drive"));
  /** Local home folder, once places are loaded. */
  homeFolder = $derived(this.places.find((p) => p.kind === "home")?.path ?? null);

  #pending = new Map<string, Promise<void>>();
  #listeners = new Set<StatusListener>();
  /** Connecting in the background (startup): prompts are declined, not shown. */
  #quiet = new Set<string>();
  /** Background connects that stopped at a prompt: they need the user. */
  needsUser = $state.raw<ReadonlySet<string>>(new Set());

  async init() {
    await api.on("conn:status", ({ host, status }) => this.#setStatus(host, status));
    const [list, places] = await Promise.all([api.listHosts(), api.localPlaces()]);
    this.list = list;
    this.places = places;
  }

  /** Re-read ~/.ssh/config, keeping statuses the backend reports. */
  async reload() {
    this.list = await api.listHosts();
  }

  get(alias: string): HostInfo | undefined {
    return this.list.find((h) => h.alias === alias);
  }

  status(alias: string): ConnStatus {
    return this.get(alias)?.status ?? { state: "disconnected" };
  }

  isConnected(alias: string): boolean {
    return this.status(alias).state === "connected";
  }

  /** Concurrent callers for one alias share one attempt. Rejects with VfsError.
   *  `quiet`: non-interactive — the backend never prompts and fails with
   *  "sign_in_needed" when it would have to (startup auto-connect). */
  connect(alias: string, quiet = false): Promise<void> {
    if (this.isConnected(alias)) return Promise.resolve();
    if (!quiet) {
      // The user asked: a quiet attempt still running can't sign in, so if it
      // stops at sign-in, start over interactively.
      const wasQuiet = this.#quiet.delete(alias);
      const pending = this.#pending.get(alias);
      if (pending && wasQuiet) {
        return pending.catch((e) => {
          if (toVfsError(e).code === "sign_in_needed") return this.connect(alias);
          throw e;
        });
      }
      if (this.needsUser.has(alias)) this.#setNeedsUser(alias, false);
    }
    let p = this.#pending.get(alias);
    if (!p) {
      p = api.connectHost(alias, !quiet)
        .then(() => {
          // The backend emits conn:status too; this covers a missed event.
          if (this.status(alias).state !== "connected") this.#setStatus(alias, { state: "connected" });
          // A password may have been saved to the keychain on the way.
          void this.reload().catch(() => {});
        })
        .catch((e) => {
          const s = this.status(alias).state;
          if (s === "connecting" || s === "connected") {
            this.#setStatus(alias, { state: "failed", error: toVfsError(e) });
          }
          throw e;
        })
        .finally(() => this.#pending.delete(alias));
      this.#pending.set(alias, p);
    }
    return p;
  }

  async disconnect(alias: string) {
    await api.disconnectHost(alias);
    if (this.status(alias).state !== "disconnected") this.#setStatus(alias, { state: "disconnected" });
  }

  /** Startup: connect every disconnected server at once, without prompts or
   *  toasts. Failures show as red dots with the error in the tooltip. */
  connectAllQuietly() {
    for (const h of this.list) {
      if (h.status.state !== "disconnected") continue;
      this.#quiet.add(h.alias);
      this.connect(h.alias, true)
        .catch(() => {})
        .finally(() => this.#quiet.delete(h.alias));
    }
  }

  /** `host` as in PromptRequest: an alias, or "tanla via bastion" for a jump hop. */
  isQuiet(host: string): boolean {
    return this.#quiet.has(promptAlias(host));
  }

  /** prompts store: a quiet connect hit a prompt (declined there). */
  declinedPrompt(host: string) {
    this.#setNeedsUser(promptAlias(host), true);
  }

  /** Dot state: "sign-in needed" and a prompt the user declined are "not
   *  connected", not errors. */
  dot(h: HostInfo): ConnStatus["state"] {
    if (h.status.state === "failed" && (h.status.error.code === "sign_in_needed" || h.status.error.code === "cancelled")) return "disconnected";
    return h.status.state;
  }

  #setNeedsUser(alias: string, on: boolean) {
    const next = new Set(this.needsUser);
    if (on) next.add(alias);
    else next.delete(alias);
    this.needsUser = next;
  }

  onStatus(fn: StatusListener): () => void {
    this.#listeners.add(fn);
    return () => this.#listeners.delete(fn);
  }

  #setStatus(host: string, status: ConnStatus) {
    const prev = this.get(host)?.status;
    if (status.state === "connected" && this.needsUser.has(host)) this.#setNeedsUser(host, false);
    // A quiet attempt that needs a password/host key: the user signs in later.
    if (status.state === "failed" && status.error.code === "sign_in_needed") this.#setNeedsUser(host, true);
    this.list = this.list.map((h) => (h.alias === host ? { ...h, status } : h));
    for (const fn of this.#listeners) fn(host, status, prev);
  }
}

export const hosts = new Hosts();
