<script lang="ts">
  import { untrack } from "svelte";
  import FileIcon from "./FileIcon.svelte";
  import Thumb from "./Thumb.svelte";
  import { wantsThumbnail } from "$lib/thumbs";
  import RenameField from "./RenameField.svelte";
  import { itemPointer } from "./itemPointer";
  import { scrollMemory } from "./scrollMemory";
  import { backgroundMenu, commitRename, entryMenu } from "$lib/actions";
  import type { Browser } from "$lib/stores/browser.svelte";
  import { clipboard } from "$lib/stores/clipboard.svelte";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { iconFor } from "$lib/util/filetype";
  import { sourceKey } from "$lib/util/path";

  let { b }: { b: Browser } = $props();

  const CELL_W = 100;
  const CELL_H = 104;
  const PAD = 12;
  const OVERSCAN = 2;

  let viewport = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let height = $state(0);
  let width = $state(0);

  const cols = $derived(Math.max(1, Math.floor((width - PAD * 2) / CELL_W)));
  const cellW = $derived(cols > 1 ? (width - PAD * 2) / cols : CELL_W);
  const rowCount = $derived(Math.ceil(b.items.length / cols));
  const firstRow = $derived(Math.max(0, Math.floor((scrollTop - PAD) / CELL_H) - OVERSCAN));
  const lastRow = $derived(Math.min(rowCount, Math.ceil((scrollTop + height) / CELL_H) + OVERSCAN));
  const start = $derived(firstRow * cols);
  const cells = $derived(b.items.slice(start, lastRow * cols));
  const srcKey = $derived(b.location ? sourceKey(b.location.source) : "");
  const cutPrefix = $derived(b.location ? `${srcKey}|` : "");

  const pointer = itemPointer(() => b);

  $effect(() => {
    b.gridColumns = cols;
  });

  const memory = scrollMemory(() => b, "icons");

  $effect(() => {
    const id = b.listingId;
    untrack(() => {
      const top = memory.onListing(id);
      if (viewport) viewport.scrollTop = top;
      scrollTop = viewport?.scrollTop ?? 0;
    });
  });

  $effect(() => {
    const r = b.reveal;
    if (!r || !viewport) return;
    untrack(() => {
      const i = b.index.get(r.path);
      if (i === undefined || !viewport) return;
      const top = PAD + Math.floor(i / cols) * CELL_H;
      const vh = viewport.clientHeight;
      if (top < viewport.scrollTop) viewport.scrollTop = Math.max(0, top - PAD);
      else if (top + CELL_H > viewport.scrollTop + vh) viewport.scrollTop = top + CELL_H - vh + PAD;
    });
  });

  function onBackgroundDown(e: MouseEvent) {
    const t = e.target as HTMLElement;
    if (t !== viewport && !t.classList.contains("spacer")) return;
    if (viewport && e.offsetX >= viewport.clientWidth) return; // scrollbar
    pointer.cancel();
    if (e.button === 0) b.selectNone();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="viewport"
  class:inactive={!ui.windowFocused}
  bind:this={viewport}
  bind:clientHeight={height}
  bind:clientWidth={width}
  onscroll={() => memory.save((scrollTop = viewport!.scrollTop))}
  onmousedown={onBackgroundDown}
  oncontextmenu={(e) => backgroundMenu(b, e)}
>
  <div class="spacer" style:height="{rowCount * CELL_H + PAD * 2}px" role="listbox" aria-multiselectable="true" aria-label="Files">
    {#each cells as entry, j (entry.path)}
      {@const i = start + j}
      {@const sel = b.selected.has(entry.path)}
      <!-- Keyboard is handled globally (src/lib/keyboard.ts). -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="cell"
        class:sel
        class:cut={clipboard.cutKeys.has(cutPrefix + entry.path) || entry.isHidden}
        style:left="{PAD + (i % cols) * cellW}px"
        style:top="{PAD + Math.floor(i / cols) * CELL_H}px"
        style:width="{cellW}px"
        class:drop={dnd.target?.key === cutPrefix + entry.path}
        data-drop={entry.kind === "dir" ? "dir" : undefined}
        data-drop-source={entry.kind === "dir" ? srcKey : undefined}
        data-drop-path={entry.kind === "dir" ? entry.path : undefined}
        data-path={entry.path}
        role="option"
        aria-selected={sel}
        tabindex="-1"
        onmousedown={(e) => pointer.down(e, entry)}
        onclick={(e) => pointer.click(e, entry, (e.target as HTMLElement).classList.contains("label"))}
        ondblclick={(e) => pointer.dblclick(e, entry)}
        oncontextmenu={(e) => entryMenu(b, e, entry)}
      >
        <div class="icon">
          {#if b.location && wantsThumbnail(entry, b.location)}
            <Thumb {entry} location={b.locationOf(entry)} size={64} />
          {:else}
            <FileIcon type={iconFor(entry)} size={64} link={entry.isSymlink} />
          {/if}
        </div>
        {#if b.renaming === entry.path}
          <div class="rename"><RenameField {entry} center oncommit={(name) => commitRename(b, entry, name)} oncancel={() => (b.renaming = null)} /></div>
        {:else}
          <span class="label" title={entry.name}>{entry.name}</span>
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .viewport {
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    contain: strict;
  }
  .spacer { position: relative; contain: layout; }
  .cell {
    position: absolute;
    height: 104px;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding-top: 4px;
  }
  .cell:focus-visible { box-shadow: none; }
  .icon { padding: 3px; border-radius: 7px; }
  .sel .icon { background: var(--bg-pressed); }
  .drop .icon { background: var(--accent-soft); box-shadow: inset 0 0 0 2px var(--accent); }
  .label {
    max-width: calc(100% - 8px);
    margin-top: 3px;
    padding: 1px 5px;
    border-radius: 4px;
    font-size: 12px;
    line-height: 15px;
    text-align: center;
    word-break: break-word;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .sel .label { background: var(--bg-sel); color: var(--fg-sel); }
  .inactive .sel .label { background: var(--bg-sel-inactive); color: var(--fg-sel-inactive); }
  .cut { opacity: 0.55; }
  .rename { width: calc(100% - 8px); margin-top: 3px; }
</style>
