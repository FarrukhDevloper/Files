<script lang="ts">
  import { untrack } from "svelte";
  import ChevronUp from "@lucide/svelte/icons/chevron-up";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import FileIcon from "./FileIcon.svelte";
  import Thumb from "./Thumb.svelte";
  import { wantsThumbnail } from "$lib/thumbs.svelte";
  import RenameField from "./RenameField.svelte";
  import GitBadge from "./GitBadge.svelte";
  import { gitLabel } from "$lib/util/git";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { itemPointer } from "./itemPointer";
  import { scrollMemory } from "./scrollMemory";
  import { backgroundMenu, commitRename, entryMenu } from "$lib/actions";
  import type { Browser } from "$lib/stores/browser.svelte";
  import type { SortKey } from "$lib/stores/prefs.svelte";
  import { clipboard } from "$lib/stores/clipboard.svelte";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { formatDate, formatSize } from "$lib/util/format";
  import { iconFor, kindLabel } from "$lib/util/filetype";
  import { sourceKey } from "$lib/util/path";

  let { b }: { b: Browser } = $props();

  const ROW = 28;
  const HEADER = 28;
  const PAD = 4;
  const OVERSCAN = 12;

  let viewport = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let height = $state(0);
  let width = $state(0);

  // Only the visible window of rows is in the DOM.
  const start = $derived(Math.max(0, Math.floor((scrollTop - PAD) / ROW) - OVERSCAN));
  const end = $derived(Math.min(b.items.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN));
  const rows = $derived(b.items.slice(start, end));

  const showDate = $derived(width === 0 || width > 520);
  const showKind = $derived(width === 0 || width > 720);
  const showGit = $derived(prefs.gitColumn && !!b.git && (width === 0 || width > 600));
  const columns = $derived(`minmax(0, 1fr)${showDate ? " 190px" : ""} 92px${showKind ? " 170px" : ""}${showGit ? " 96px" : ""}`);
  const srcKey = $derived(b.location ? sourceKey(b.location.source) : "");
  const cutPrefix = $derived(b.location ? `${srcKey}|` : "");

  const pointer = itemPointer(() => b);

  const memory = scrollMemory(() => b, "list");

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
      // Spacer coordinates; the sticky header hides the top HEADER px.
      const top = PAD + i * ROW;
      const vh = viewport.clientHeight - HEADER;
      if (top < viewport.scrollTop) viewport.scrollTop = top;
      else if (top + ROW > viewport.scrollTop + vh) viewport.scrollTop = top + ROW - vh;
    });
  });

  function header(key: SortKey) {
    return { active: b.sort.key === key, asc: b.sort.asc };
  }

  function onBackgroundDown(e: MouseEvent) {
    const t = e.target as HTMLElement;
    if (t !== viewport && !t.classList.contains("spacer")) return;
    if (viewport && e.offsetX >= viewport.clientWidth) return; // scrollbar
    pointer.cancel();
    if (e.button === 0) b.selectNone();
  }
</script>

{#snippet sortHeader(key: SortKey, label: string, align = "left")}
  {@const h = header(key)}
  <button class="col {align}" class:active={h.active} onclick={() => b.sortBy(key)}>
    <span>{label}</span>
    {#if h.active}
      <span class="arrow">{#if h.asc}<ChevronUp size={12} strokeWidth={2.2} />{:else}<ChevronDown size={12} strokeWidth={2.2} />{/if}</span>
    {/if}
  </button>
{/snippet}

<!-- The header lives inside the scroller (sticky) so it shares the rows'
     width — a vertical scrollbar can't shift the columns out of line. -->
<div class="list" class:inactive={!ui.windowFocused || tabs.current !== b} style:--cols={columns}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="viewport"
    bind:this={viewport}
    bind:clientHeight={height}
    bind:clientWidth={width}
    onscroll={() => memory.save((scrollTop = viewport!.scrollTop))}
    onmousedown={onBackgroundDown}
    oncontextmenu={(e) => backgroundMenu(b, e)}
  >
    <div class="header" role="row">
      {@render sortHeader("name", "Name")}
      {#if showDate}{@render sortHeader("modified", "Date Modified")}{/if}
      {@render sortHeader("size", "Size", "right")}
      {#if showKind}{@render sortHeader("kind", "Kind")}{/if}
      {#if showGit}<div class="col">Git</div>{/if}
    </div>
    <div class="spacer" style:height="{b.items.length * ROW + PAD * 2}px" role="listbox" aria-multiselectable="true" aria-label="Files">
      {#each rows as entry, j (entry.path)}
        {@const i = start + j}
        {@const sel = b.selected.has(entry.path)}
        {@const prevSel = sel && i > 0 && b.selected.has(b.items[i - 1]!.path)}
        {@const nextSel = sel && b.selected.has(b.items[i + 1]?.path ?? "")}
        <!-- Keyboard is handled globally (src/lib/keyboard.ts). -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="row"
          class:alt={i % 2 === 1}
          class:sel
          class:join-top={prevSel}
          class:join-bottom={nextSel}
          class:cut={clipboard.cutKeys.has(cutPrefix + entry.path) || b.pending.has(entry.path)}
          class:hidden-file={entry.isHidden}
          class:fresh={b.fresh.has(entry.path)}
          class:leaving={b.leaving.has(entry.path)}
          style:top="{PAD + i * ROW}px"
          class:drop={dnd.target?.key === cutPrefix + entry.path}
          data-drop={entry.kind === "dir" ? "dir" : undefined}
          data-drop-source={entry.kind === "dir" ? srcKey : undefined}
          data-drop-path={entry.kind === "dir" ? entry.path : undefined}
          data-path={entry.path}
          role="option"
          aria-selected={sel}
          tabindex="-1"
          onmousedown={(e) => pointer.down(e, entry)}
          onclick={(e) => pointer.click(e, entry, (e.target as HTMLElement).classList.contains("name-text"))}
          ondblclick={(e) => pointer.dblclick(e, entry)}
          oncontextmenu={(e) => entryMenu(b, e, entry)}
        >
          <div class="cell name">
            {#if b.location && wantsThumbnail(entry, b.location, true)}
              <Thumb {entry} location={b.locationOf(entry)} size={20} />
            {:else}
              <FileIcon type={iconFor(entry)} size={20} link={entry.isSymlink} />
            {/if}
            {#if b.renaming === entry.path}
              <RenameField {entry} oncommit={(name) => commitRename(b, entry, name)} oncancel={() => (b.renaming = null)} />
            {:else}
              <span class="name-text">{entry.name}</span>
              {#if b.git?.entries[entry.name] && !showGit}<GitBadge status={b.git.entries[entry.name]!} />{/if}
            {/if}
          </div>
          {#if showDate}<div class="cell muted">{formatDate(entry.modified)}</div>{/if}
          <div class="cell muted right">{entry.kind === "dir" ? "—" : formatSize(entry.size)}</div>
          {#if showKind}<div class="cell muted">{kindLabel(entry)}</div>{/if}
          {#if showGit}
            {@const st = b.git?.entries[entry.name]}
            <div class="cell git-col {st ?? ''}">{st ? gitLabel[st] : ""}</div>
          {/if}
        </div>
      {/each}
    </div>
  </div>
</div>

<style>
  .list {
    position: relative;
    height: 100%;
    min-width: 0;
  }
  /* Header and rows use the same box: 8px inset each side, one grid. */
  .header, .row {
    display: grid;
    grid-template-columns: var(--cols);
    margin: 0 8px;
  }
  .header {
    position: sticky;
    top: 0;
    z-index: 1;
    height: 28px;
    background: var(--bg-window);
    box-shadow: 0 1px 0 var(--separator), -8px 0 0 var(--bg-window), 8px 0 0 var(--bg-window);
    font-size: var(--fs-small);
    color: var(--fg-secondary);
  }
  .col {
    position: relative;
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    padding: 0 8px;
    white-space: nowrap;
    overflow: hidden;
    font-weight: 500;
  }
  .col:first-child { padding-left: 6px; }
  .col + .col::before {
    content: "";
    position: absolute;
    left: 0;
    top: 6px;
    bottom: 6px;
    width: 1px;
    background: var(--separator);
  }
  .col:hover { color: var(--fg-primary); }
  .col.active { color: var(--fg-primary); font-weight: 600; }
  .col.right { justify-content: flex-end; }
  .col.right .arrow { order: -1; }
  .arrow { display: grid; place-items: center; }

  .viewport {
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    contain: strict;
  }
  .spacer { position: relative; contain: layout; }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: var(--row-h);
    align-items: center;
    border-radius: 5px;
  }
  .row:focus-visible { box-shadow: none; }
  .row.alt { background: var(--bg-row-alt); }
  .row.sel { background: var(--bg-sel); color: var(--fg-sel); }
  .row.sel .muted { color: inherit; opacity: 0.85; }
  .row.join-top { border-top-left-radius: 0; border-top-right-radius: 0; }
  .row.join-bottom { border-bottom-left-radius: 0; border-bottom-right-radius: 0; }
  .inactive .row.sel { background: var(--bg-sel-inactive); color: var(--fg-sel-inactive); }
  .row.drop { background: var(--accent-soft); box-shadow: inset 0 0 0 2px var(--accent); }
  .row.cut .cell, .row.hidden-file .cell { opacity: 0.55; }
  .row.fresh { animation: row-in var(--dur) var(--ease-out); }
  .row.leaving { opacity: 0; transform: translateX(-8px); transition: opacity 0.14s var(--ease-out), transform 0.14s var(--ease-out); }
  @keyframes row-in { from { opacity: 0; transform: translateY(-6px); } }

  .cell {
    min-width: 0;
    padding: 0 8px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-variant-numeric: tabular-nums;
  }
  .cell.name { display: flex; align-items: center; gap: 7px; padding-left: 6px; }
  .name-text { overflow: hidden; text-overflow: ellipsis; }
  .muted { color: var(--fg-secondary); }
  .cell.name :global(.git) { margin-left: 2px; }
  .row.sel .cell.name :global(.git) { color: inherit; }
  .git-col { font-weight: 550; }
  .git-col.modified { color: var(--git-modified); }
  .git-col.added { color: var(--git-added); }
  .git-col.untracked { color: var(--git-untracked); }
  .git-col.renamed { color: var(--git-renamed); }
  .git-col.deleted, .git-col.conflicted { color: var(--git-deleted); }
  .row.sel .git-col { color: inherit; }
  .right { text-align: right; }
</style>
