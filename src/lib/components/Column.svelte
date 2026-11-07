<!-- One virtualized column of the column (Miller) view. Pure display: the
     parent decides what clicks mean for this column. -->
<script lang="ts">
  import { untrack } from "svelte";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import FileIcon from "./FileIcon.svelte";
  import Thumb from "./Thumb.svelte";
  import { wantsThumbnail } from "$lib/thumbs.svelte";
  import RenameField from "./RenameField.svelte";
  import GitBadge from "./GitBadge.svelte";
  import type { Entry, GitFileStatus, SourceId } from "$lib/ipc/types";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { iconFor } from "$lib/util/filetype";

  interface Props {
    entries: Entry[];
    srcKey: string;
    /** Where `entries` live; enables row thumbnails. */
    source?: SourceId | null;
    /** Accent selection (focused column) or grey (path / unfocused). */
    active: boolean;
    isSelected: (path: string) => boolean;
    isCut?: (path: string) => boolean;
    /** Insert / remove animations (current column only). */
    isFresh?: (path: string) => boolean;
    isLeaving?: (path: string) => boolean;
    /** Git status by entry name (current column). */
    gitStatus?: (name: string) => GitFileStatus | undefined;
    renaming?: string | null;
    reveal?: string | null;
    revealSeq?: number;
    onrowdown: (e: MouseEvent, entry: Entry) => void;
    onrowclick?: (e: MouseEvent, entry: Entry) => void;
    onrowdbl?: (e: MouseEvent, entry: Entry) => void;
    onrowmenu: (e: MouseEvent, entry: Entry) => void;
    onbackground?: (e: MouseEvent) => void;
    onrename?: (entry: Entry, name: string) => void;
    oncancelrename?: () => void;
  }

  let {
    entries, srcKey, source = null, active, isSelected, isCut = () => false, isFresh = () => false, isLeaving = () => false, gitStatus = () => undefined, renaming = null, reveal = null, revealSeq = 0,
    onrowdown, onrowclick, onrowdbl, onrowmenu, onbackground, onrename, oncancelrename,
  }: Props = $props();

  const ROW = 26;
  const PAD = 4;
  const OVERSCAN = 10;

  let el = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let height = $state(0);

  const start = $derived(Math.max(0, Math.floor((scrollTop - PAD) / ROW) - OVERSCAN));
  const end = $derived(Math.min(entries.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN));
  const rows = $derived(entries.slice(start, end));

  $effect(() => {
    const path = reveal;
    void revealSeq;
    if (!path || !el) return;
    untrack(() => {
      const i = entries.findIndex((e) => e.path === path);
      if (i < 0 || !el) return;
      const top = PAD + i * ROW;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (top + ROW > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW - el.clientHeight;
    });
  });

  function onmousedown(e: MouseEvent) {
    const t = e.target as HTMLElement;
    if ((t === el || t.classList.contains("spacer")) && e.offsetX < (el?.clientWidth ?? 0)) onbackground?.(e);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="column-scroll" bind:this={el} bind:clientHeight={height} onscroll={() => (scrollTop = el!.scrollTop)} {onmousedown}>
  <div class="spacer" style:height="{entries.length * ROW + PAD * 2}px" role="listbox" aria-multiselectable="true">
    {#each rows as entry, j (entry.path)}
      {@const i = start + j}
      {@const sel = isSelected(entry.path)}
      {@const dir = entry.kind === "dir"}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="row"
        class:sel
        class:active={sel && active}
        class:cut={isCut(entry.path) || entry.isHidden}
        class:fresh={isFresh(entry.path)}
        class:leaving={isLeaving(entry.path)}
        class:drop={dnd.target?.key === `${srcKey}|${entry.path}`}
        style:top="{PAD + i * ROW}px"
        data-path={entry.path}
        role="option"
        aria-selected={sel}
        tabindex="-1"
        data-drop={dir ? "dir" : undefined}
        data-drop-source={dir ? srcKey : undefined}
        data-drop-path={dir ? entry.path : undefined}
        onmousedown={(e) => onrowdown(e, entry)}
        onclick={(e) => onrowclick?.(e, entry)}
        ondblclick={(e) => onrowdbl?.(e, entry)}
        oncontextmenu={(e) => onrowmenu(e, entry)}
      >
        {#if source && wantsThumbnail(entry, { source, path: entry.path }, true)}
          <Thumb {entry} location={{ source, path: entry.path }} size={18} />
        {:else}
          <FileIcon type={iconFor(entry)} size={18} link={entry.isSymlink} />
        {/if}
        {#if renaming === entry.path && onrename}
          <RenameField {entry} oncommit={(name) => onrename(entry, name)} oncancel={() => oncancelrename?.()} />
        {:else}
          <span class="name name-text">{entry.name}</span>
          {@const st = gitStatus(entry.name)}
          {#if st}<GitBadge status={st} />{/if}
        {/if}
        {#if dir}<span class="chev"><ChevronRight size={13} strokeWidth={2.2} /></span>{/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .column-scroll {
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    contain: strict;
  }
  .spacer { position: relative; contain: layout; }
  .row {
    position: absolute;
    left: 6px;
    right: 6px;
    height: 26px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 4px 0 6px;
    border-radius: 5px;
  }
  .row:focus-visible { box-shadow: none; }
  .row.sel { background: var(--bg-sel-inactive); color: var(--fg-sel-inactive); }
  .row.active { background: var(--bg-sel); color: var(--fg-sel); }
  .row.drop { background: var(--accent-soft); box-shadow: inset 0 0 0 2px var(--accent); }
  .row.cut .name, .row.cut :global(.fi) { opacity: 0.55; }
  .row.fresh { animation: row-in var(--dur) var(--ease-out); }
  .row.leaving { opacity: 0; transform: translateX(-8px); transition: opacity 0.14s var(--ease-out), transform 0.14s var(--ease-out); }
  @keyframes row-in { from { opacity: 0; transform: translateY(-6px); } }
  .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chev { display: grid; place-items: center; color: var(--fg-tertiary); }
  .row.sel .chev { color: inherit; }
  .row.active :global(.git) { color: inherit; }
</style>
