// Transfer jobs mirrored from `transfer:update` events.

import { api } from "$lib/ipc/api";
import type { JobSnapshot, JobState, TransferRequest } from "$lib/ipc/types";
import { toasts } from "./toasts.svelte";

export function isFinished(state: JobState): boolean {
  return state === "completed" || state === "failed" || state === "cancelled";
}

type SettledListener = (job: JobSnapshot) => void;

class Transfers {
  /** Newest first. */
  jobs = $state.raw<JobSnapshot[]>([]);
  active = $derived(this.jobs.filter((j) => !isFinished(j.state)));
  hasFinished = $derived(this.jobs.some((j) => isFinished(j.state)));
  /** 0..1 across all unfinished jobs, for the toolbar button. */
  progress = $derived.by(() => {
    let total = 0, done = 0;
    for (const j of this.active) { total += j.totalBytes; done += j.doneBytes; }
    return total > 0 ? done / total : 0;
  });

  #settled = new Set<SettledListener>();

  async init() {
    await api.on("transfer:update", (job) => this.#upsert(job));
    const list = await api.transferList();
    for (const job of list) this.#upsert(job, false);
  }

  onSettled(fn: SettledListener): () => void {
    this.#settled.add(fn);
    return () => this.#settled.delete(fn);
  }

  async start(request: TransferRequest): Promise<string | null> {
    try {
      return await api.transferStart(request);
    } catch (e) {
      toasts.error(e, request.op === "move" ? "Couldn't start moving" : "Couldn't start copying");
      return null;
    }
  }

  async pause(id: string) {
    await api.transferPause(id).catch((e) => toasts.error(e, "Couldn't pause the transfer"));
  }

  async resume(id: string) {
    await api.transferResume(id).catch((e) => toasts.error(e, "Couldn't resume the transfer"));
  }

  async cancel(id: string) {
    await api.transferCancel(id).catch((e) => toasts.error(e, "Couldn't cancel the transfer"));
  }

  async clearFinished() {
    try {
      await api.transferClearFinished();
      this.jobs = this.jobs.filter((j) => !isFinished(j.state));
    } catch (e) {
      toasts.error(e, "Couldn't clear transfers");
    }
  }

  #upsert(job: JobSnapshot, notify = true) {
    const i = this.jobs.findIndex((j) => j.id === job.id);
    const prev = i >= 0 ? this.jobs[i] : undefined;
    if (prev) {
      const next = this.jobs.slice();
      next[i] = job;
      this.jobs = next;
    } else {
      this.jobs = [job, ...this.jobs].sort((a, b) => b.createdAt - a.createdAt);
    }
    if (notify && isFinished(job.state) && (!prev || !isFinished(prev.state))) {
      for (const fn of this.#settled) fn(job);
    }
  }
}

export const transfers = new Transfers();
