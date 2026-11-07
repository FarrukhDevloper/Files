// SSH hosts from ~/.ssh/config, their live connection status, and the local
// places shown in the sidebar.

import { api } from "$lib/ipc/api";
import type { ConnStatus, HostInfo, Place } from "$lib/ipc/types";
import { toVfsError } from "$lib/util/errors";

type StatusListener = (host: string, status: ConnStatus, prev: ConnStatus | undefined) => void;

class Hosts {
  list = $state.raw<HostInfo[]>([]);
  places = $state.raw<Place[]>([]);
  favourites = $derived(this.places.filter((p) => p.kind !== "root" && p.kind !== "drive"));
  locations = $derived(this.places.filter((p) => p.kind === "root" || p.kind === "drive"));
  /** Local home folder, once places are loaded. */
  homeFolder = $derived(this.places.find((p) => p.kind === "home")?.path ?? null);

  #pending = new Map<string, Promise<void>>();
  #listeners = new Set<StatusListener>();

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

  /** Concurrent callers for one alias share one attempt. Rejects with VfsError. */
  connect(alias: string): Promise<void> {
    if (this.isConnected(alias)) return Promise.resolve();
    let p = this.#pending.get(alias);
    if (!p) {
      p = api.connectHost(alias)
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

  onStatus(fn: StatusListener): () => void {
    this.#listeners.add(fn);
    return () => this.#listeners.delete(fn);
  }

  #setStatus(host: string, status: ConnStatus) {
    const prev = this.get(host)?.status;
    this.list = this.list.map((h) => (h.alias === host ? { ...h, status } : h));
    for (const fn of this.#listeners) fn(host, status, prev);
  }
}

export const hosts = new Hosts();
