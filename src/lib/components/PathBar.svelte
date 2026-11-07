<script lang="ts">
  import Laptop from "@lucide/svelte/icons/laptop-minimal";
  import Server from "@lucide/svelte/icons/server";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import FileIcon from "./FileIcon.svelte";
  import { api } from "$lib/ipc/api";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { formatSize, plural } from "$lib/util/format";
  import { ancestors, baseName, sourceKey, sourceLabel } from "$lib/util/path";

  const b = $derived(tabs.current);
  const crumbs = $derived(b.location ? ancestors(b.location.path) : []);

  const status = $derived.by(() => {
    if (b.error) return "";
    const n = b.items.length;
    const sel = b.selected.size;
    if (b.filter) {
      const total = b.showHidden ? b.entries.length : b.entries.filter((e) => !e.isHidden).length;
      return `${plural(n, "match", "matches")} of ${plural(total, "item")}`;
    }
    if (sel > 0) return `${sel} of ${plural(n, "item")} selected`;
    return plural(n, "item");
  });

  // Free space for the shown volume; re-read whenever the listing changes
  // (navigation, refresh, finished transfer). null = unknown (e.g. remote).
  let free = $state<number | null>(null);
  $effect(() => {
    const loc = b.location;
    void b.entries;
    if (!loc || b.error) {
      free = null;
      return;
    }
    let live = true;
    api.diskFree(loc).then(
      (d) => live && (free = d ? d.free : null),
      () => live && (free = null),
    );
    return () => { live = false; };
  });

  // Delay the spinner so fast listings don't flash it.
  let showSpinner = $state(false);
  $effect(() => {
    if (!b.loading && !b.refreshing) {
      showSpinner = false;
      return;
    }
    const t = setTimeout(() => (showSpinner = true), 200);
    return () => clearTimeout(t);
  });

  function go(path: string) {
    if (b.location && path !== b.location.path) b.go({ source: b.location.source, path });
  }
</script>

<footer class="pathbar">
  <div class="crumbs">
    {#if b.location}
      {#each crumbs as path, i (path)}
        {#if i > 0}<span class="sep"><ChevronRight size={11} strokeWidth={2.2} /></span>{/if}
        {@const key = `${sourceKey(b.location.source)}|${path}`}
        <button
          class="crumb"
          class:last={i === crumbs.length - 1}
          class:drop={dnd.target?.key === key}
          data-drop="dir"
          data-drop-source={sourceKey(b.location.source)}
          data-drop-path={path}
          onclick={() => go(path)}
          title={path}
        >
          {#if i === 0}
            {#if b.location.source.kind === "local"}<Laptop size={13} strokeWidth={1.8} />{:else}<Server size={13} strokeWidth={1.8} />{/if}
            <span>{sourceLabel(b.location.source)}</span>
          {:else}
            <FileIcon type="folder" size={14} />
            <span>{baseName(path)}</span>
          {/if}
        </button>
      {/each}
    {/if}
  </div>
  <div class="status">
    {#if showSpinner}<span class="spinner" aria-label="Loading"></span>{/if}
    <span>{status}{free !== null ? `, ${formatSize(free)} available` : ""}</span>
  </div>
</footer>

<style>
  .pathbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    height: var(--pathbar-h);
    padding: 0 12px 0 8px;
    background: var(--bg-pathbar);
    border-top: 1px solid var(--border);
    font-size: var(--fs-small);
    color: var(--fg-secondary);
  }
  .crumbs {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    overflow: hidden;
    white-space: nowrap;
  }
  .crumb {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    flex-shrink: 1;
    height: 20px;
    padding: 0 4px;
    border-radius: 4px;
    color: inherit;
  }
  .crumb span { overflow: hidden; text-overflow: ellipsis; }
  .crumb:hover { background: var(--bg-hover); color: var(--fg-primary); }
  .crumb.last { color: var(--fg-primary); flex-shrink: 0; }
  .crumb.drop { background: var(--accent); color: var(--accent-fg); }
  .sep { display: grid; place-items: center; color: var(--fg-tertiary); flex: none; }
  .status { display: flex; align-items: center; gap: 6px; flex: none; }
</style>
