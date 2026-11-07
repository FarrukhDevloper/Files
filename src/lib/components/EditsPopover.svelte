<!-- Remote files open in local apps: sync state per file, Stop, Resolve. -->
<script lang="ts">
  import { untrack } from "svelte";
  import X from "@lucide/svelte/icons/x";
  import FileIcon from "./FileIcon.svelte";
  import type { EditSession } from "$lib/ipc/types";
  import { edits } from "$lib/stores/edits.svelte";
  import { rememberFocus, trapTab } from "$lib/util/focus";
  import { iconFor } from "$lib/util/filetype";
  import { baseName, parentPath, sourceLabel } from "$lib/util/path";

  let el = $state<HTMLDivElement>();

  function where(s: EditSession): string {
    return `${sourceLabel(s.location.source)} · ${parentPath(s.location.path) ?? "/"}`;
  }

  function time(ms: number): string {
    return new Date(ms).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
  }

  function detail(s: EditSession): string {
    switch (s.state) {
      case "downloading": return "Downloading…";
      case "open": return s.lastSync ? `Saved to server at ${time(s.lastSync)}` : "Open — saves are uploaded";
      case "uploading": return "Uploading your changes…";
      case "synced": return `Saved to server at ${time(s.lastSync ?? Date.now())}`;
      case "conflict": return "Changed on the server too";
      case "failed": return s.error?.message ?? "Sync failed";
      case "closed": return "Closed";
    }
  }

  function resolve(s: EditSession) {
    edits.open = false;
    edits.askConflict(s);
  }

  function onpointerdown(e: PointerEvent) {
    const t = e.target as HTMLElement;
    if (el && !el.contains(t) && !t.closest(".edits")) edits.open = false;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      edits.open = false;
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

<div class="popover" bind:this={el} role="dialog" aria-label="Files open in apps" tabindex="-1">
  <div class="head">
    <h2>Open in Apps</h2>
    <p>Each save is uploaded back to the server.</p>
  </div>
  {#if edits.list.length === 0}
    <p class="empty">Nothing open. Double-click a file on a server to open it in its app.</p>
  {:else}
    <ul>
      {#each edits.list as s (s.id)}
        {@const name = baseName(s.location.path)}
        <li class={s.state}>
          <FileIcon type={iconFor({ name, kind: "file" })} size={32} />
          <div class="main">
            <div class="name" title={s.localPath}>{name}</div>
            <div class="where">{where(s)}</div>
            <div class="detail">
              {#if s.state === "downloading" || s.state === "uploading"}<span class="spinner"></span>{/if}
              <span>{detail(s)}</span>
              {#if s.state === "conflict"}<button class="link" onclick={() => resolve(s)}>Resolve…</button>{/if}
            </div>
          </div>
          <button class="tool stop" title="Stop syncing and delete the local copy" aria-label="Stop syncing {name}" onclick={() => void edits.stop(s.id)}>
            <X size={14} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .popover {
    position: absolute;
    top: calc(100% + 8px);
    right: -120px;
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
  .head { padding: 10px 14px 6px; }
  h2 { margin: 0; font-size: var(--fs-ui); font-weight: 650; }
  .head p { margin: 2px 0 0; color: var(--fg-secondary); font-size: var(--fs-small); }
  .empty { margin: 4px 14px 14px; color: var(--fg-secondary); font-size: var(--fs-secondary); }
  ul { list-style: none; margin: 0; padding: 0 6px 6px; overflow-y: auto; }
  li { display: flex; align-items: center; gap: 10px; padding: 8px; border-radius: 7px; }
  li + li { border-top: 1px solid var(--separator); }
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .name, .where { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .name { font-weight: 550; }
  .where { font-size: var(--fs-small); color: var(--fg-secondary); }
  .detail { display: flex; align-items: center; gap: 6px; font-size: var(--fs-small); color: var(--fg-secondary); }
  .detail .spinner { width: 10px; height: 10px; }
  .synced .detail, .open .detail { color: var(--fg-secondary); }
  .conflict .detail, .failed .detail { color: var(--danger); }
  .link { color: var(--accent); font-weight: 550; }
  .stop { flex: none; height: 24px; min-width: 24px; padding: 0; border-radius: 50%; }
  @keyframes pop { from { transform: translateY(-4px) scale(0.97); opacity: 0; } }
</style>
