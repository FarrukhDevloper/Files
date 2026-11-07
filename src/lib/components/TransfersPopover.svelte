<script lang="ts">
  import { untrack } from "svelte";
  import Pause from "@lucide/svelte/icons/pause";
  import Play from "@lucide/svelte/icons/play";
  import X from "@lucide/svelte/icons/x";
  import ArrowRight from "@lucide/svelte/icons/arrow-right";
  import FileIcon from "./FileIcon.svelte";
  import type { JobSnapshot } from "$lib/ipc/types";
  import { isFinished, transfers } from "$lib/stores/transfers.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { rememberFocus, trapTab } from "$lib/util/focus";
  import { formatEta, formatRate, formatSize, plural } from "$lib/util/format";
  import { iconFor } from "$lib/util/filetype";
  import { baseName, sourceLabel } from "$lib/util/path";

  let el = $state<HTMLDivElement>();

  function title(j: JobSnapshot): string {
    const first = j.sources[0];
    const name = first ? baseName(first.path) || sourceLabel(first.source) : "items";
    return j.sources.length > 1 ? `${name} and ${plural(j.sources.length - 1, "other")}` : name;
  }

  function destination(j: JobSnapshot): string {
    const name = baseName(j.destDir.path) || "/";
    return `${sourceLabel(j.destDir.source)} · ${name}`;
  }

  function detail(j: JobSnapshot): string {
    switch (j.state) {
      case "queued": return "Waiting…";
      case "scanning": return `Preparing… ${plural(j.totalFiles, "file")}`;
      case "paused": return `Paused — ${formatSize(j.doneBytes)} of ${formatSize(j.totalBytes)}`;
      case "completed": return `${j.op === "move" ? "Moved" : "Copied"} ${formatSize(j.totalBytes)}${j.totalFiles > 1 ? ` · ${plural(j.totalFiles, "file")}` : ""}`;
      case "cancelled": return "Cancelled";
      case "failed": return j.error?.message ?? "Failed";
      case "running": {
        const parts = [`${formatSize(j.doneBytes)} of ${formatSize(j.totalBytes)}`];
        if (j.bytesPerSec > 0) {
          parts.push(formatRate(j.bytesPerSec));
          const eta = formatEta((j.totalBytes - j.doneBytes) / j.bytesPerSec);
          if (eta) parts.push(eta);
        }
        return parts.join(" — ");
      }
    }
  }

  function iconOf(j: JobSnapshot) {
    const first = j.sources[0];
    const name = first ? baseName(first.path) : "";
    // Without a stat we can't tell folder from file; a dot is a good hint.
    return iconFor({ name, kind: name.includes(".") ? "file" : "dir" });
  }

  function onpointerdown(e: PointerEvent) {
    const t = e.target as HTMLElement;
    if (el && !el.contains(t) && !t.closest(".transfers")) ui.transfersOpen = false;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      ui.transfersOpen = false;
    } else if (el?.contains(document.activeElement)) {
      trapTab(e, el);
    }
  }

  $effect(() => {
    const restore = rememberFocus();
    untrack(() => el)?.focus({ preventScroll: true });
    return restore;
  });
</script>

<svelte:document onpointerdowncapture={onpointerdown} />
<svelte:window onkeydowncapture={onkeydown} />

<div class="popover" bind:this={el} role="dialog" aria-label="Transfers" tabindex="-1">
  <div class="head">
    <h2>Transfers</h2>
    <button class="link" disabled={!transfers.hasFinished} onclick={() => transfers.clearFinished()}>Clear</button>
  </div>
  {#if transfers.jobs.length === 0}
    <p class="empty">No transfers. Copy or cut items, then paste them into a folder on any computer or server.</p>
  {:else}
    <ul>
      {#each transfers.jobs as j (j.id)}
        {@const done = isFinished(j.state)}
        <li class:done class:failed={j.state === "failed"}>
          <FileIcon type={iconOf(j)} size={32} />
          <div class="main">
            <div class="name">{title(j)}</div>
            <div class="dest">{j.op === "move" ? "Move" : "Copy"} <ArrowRight size={10} strokeWidth={2.2} /> {destination(j)}</div>
            {#if !done}
              <div class="progress" class:paused={j.state === "paused"}>
                <span style:transform="scaleX({j.totalBytes ? j.doneBytes / j.totalBytes : 0})"></span>
              </div>
            {/if}
            <div class="detail" title={j.current ?? ""}>{detail(j)}</div>
          </div>
          {#if !done}
            <div class="actions">
              {#if j.state === "paused"}
                <button class="tool" title="Resume" aria-label="Resume" onclick={() => transfers.resume(j.id)}><Play size={13} /></button>
              {:else}
                <button class="tool" title="Pause" aria-label="Pause" disabled={j.state !== "running"} onclick={() => transfers.pause(j.id)}><Pause size={13} /></button>
              {/if}
              <button class="tool" title="Cancel" aria-label="Cancel" onclick={() => transfers.cancel(j.id)}><X size={14} /></button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .popover {
    position: absolute;
    top: calc(100% + 8px);
    right: -90px;
    z-index: 30;
    width: 380px;
    max-height: min(480px, calc(100vh - 120px));
    display: flex;
    flex-direction: column;
    border-radius: var(--r-popover);
    background: var(--bg-popover);
    box-shadow: var(--shadow-popover);
    transform-origin: top right;
    animation: pop var(--dur-fast) var(--ease-out);
  }
  .popover:focus-visible { outline: none; }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px 6px;
  }
  h2 { margin: 0; font-size: var(--fs-ui); font-weight: 650; }
  .link { color: var(--accent); font-size: var(--fs-secondary); }
  .link:disabled { color: var(--fg-tertiary); }
  .empty { margin: 4px 14px 14px; color: var(--fg-secondary); font-size: var(--fs-secondary); }
  ul { list-style: none; margin: 0; padding: 0 6px 6px; overflow-y: auto; }
  li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px;
    border-radius: 7px;
    transition: opacity 0.4s;
  }
  li + li { border-top: 1px solid var(--separator); }
  li.done { opacity: 0.6; }
  li.failed { opacity: 1; }
  li.failed .detail { color: var(--danger); }
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .name, .detail, .dest { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .name { font-weight: 550; }
  .dest { display: flex; align-items: center; gap: 3px; font-size: var(--fs-small); color: var(--fg-secondary); }
  .detail { font-size: var(--fs-small); color: var(--fg-secondary); font-variant-numeric: tabular-nums; }
  .progress { height: 4px; margin: 3px 0 1px; border-radius: 2px; background: var(--progress-track); overflow: hidden; }
  .progress span {
    display: block;
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transform-origin: left;
    /* Updates come ≤ 5×/s: glide between them instead of stepping. */
    transition: transform 0.2s linear;
  }
  .progress.paused span { background: var(--fg-tertiary); }
  .actions { display: flex; flex: none; }
  .actions .tool { height: 24px; min-width: 24px; padding: 0; border-radius: 50%; }
  @keyframes pop { from { transform: translateY(-4px) scale(0.97); opacity: 0; } }
</style>
