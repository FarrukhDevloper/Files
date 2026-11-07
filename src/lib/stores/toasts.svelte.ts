// Transient notifications, mostly errors keyed by VfsError.code.

import { errorHint, errorTitle, toVfsError } from "$lib/util/errors";

export interface ToastAction { label: string; run: () => void }
export interface Toast {
  id: number;
  tone: "error" | "info";
  title: string;
  message?: string;
  action?: ToastAction;
}

const MAX = 4;

class Toasts {
  list = $state.raw<Toast[]>([]);
  #next = 1;
  #timers = new Map<number, ReturnType<typeof setTimeout>>();

  push(t: Omit<Toast, "id">, ttl = t.tone === "error" ? 8000 : 4000): number {
    const dup = this.list.find((x) => x.title === t.title && x.message === t.message);
    if (dup) this.dismiss(dup.id);
    const id = this.#next++;
    const list = [...this.list, { ...t, id }];
    for (const old of list.slice(0, Math.max(0, list.length - MAX))) this.#clear(old.id);
    this.list = list.slice(-MAX);
    this.#timers.set(id, setTimeout(() => this.dismiss(id), ttl));
    return id;
  }

  info(title: string, message?: string) {
    this.push({ tone: "info", title, message });
  }

  /** Show a failed IPC call. `context` says what was being attempted. */
  error(e: unknown, context?: string, action?: ToastAction) {
    const err = toVfsError(e);
    if (err.code === "cancelled") return;
    if (context) {
      this.push({ tone: "error", title: context, message: `${errorTitle(err.code)}. ${errorHint(err)}`, action });
    } else {
      this.push({ tone: "error", title: errorTitle(err.code), message: errorHint(err), action });
    }
  }

  /** Keep a toast up while the pointer is over it. */
  hold(id: number) {
    this.#clear(id);
  }

  release(id: number) {
    if (this.list.some((t) => t.id === id)) this.#timers.set(id, setTimeout(() => this.dismiss(id), 3000));
  }

  dismiss(id: number) {
    this.#clear(id);
    this.list = this.list.filter((t) => t.id !== id);
  }

  #clear(id: number) {
    clearTimeout(this.#timers.get(id));
    this.#timers.delete(id);
  }
}

export const toasts = new Toasts();
