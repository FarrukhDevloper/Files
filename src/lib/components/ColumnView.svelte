<!-- Column (Miller) view: one column per folder from the root down to the
     current folder, plus a column for the selected subfolder (or an info
     pane for a selected file). The current folder is `b.location`; clicking
     in another column moves `b` there and selects the clicked item. -->
<script lang="ts">
  import { untrack } from "svelte";
  import Column from "./Column.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Thumb from "./Thumb.svelte";
  import { wantsThumbnail } from "$lib/thumbs.svelte";
  import { itemPointer } from "./itemPointer";
  import { backgroundMenu, commitRename, entryMenu, openEntry } from "$lib/actions";
  import type { Entry, Location } from "$lib/ipc/types";
  import { sortEntries, type Browser } from "$lib/stores/browser.svelte";
  import { clipboard } from "$lib/stores/clipboard.svelte";
  import { listings, sameListing } from "$lib/stores/listings";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { toVfsError, errorTitle } from "$lib/util/errors";
  import { formatDate, formatSize } from "$lib/util/format";
  import { iconFor, kindLabel } from "$lib/util/filetype";
  import { ancestors, sourceKey } from "$lib/util/path";

  let { b }: { b: Browser } = $props();

  const PREVIEW_DELAY = 120;

  const srcKey = $derived(b.location ? sourceKey(b.location.source) : "");
  const chain = $derived(b.location ? ancestors(b.location.path) : []);
  const parents = $derived(chain.slice(0, -1));

  function shown(list: readonly Entry[]): Entry[] {
    return sortEntries(b.showHidden ? list : list.filter((e) => !e.isHidden), b.sort);
  }

  // ---- ancestor listings: shown from the listing cache at once, then
  // re-listed (again after a refresh); cleared when the source changes ----

  let cache = $state.raw(new Map<string, Entry[] | { error: string }>());
  let cacheSrc = "";
  let cacheKey = "";
  const pending = new Set<string>();

  $effect(() => {
    const key = `${srcKey}#${b.refreshCount}`;
    const need = parents;
    const loc = b.location;
    if (!loc) return;
    untrack(() => {
      const fresh = key !== cacheKey;
      if (srcKey !== cacheSrc) {
        cacheSrc = srcKey;
        cache = new Map();
      }
      if (fresh) {
        cacheKey = key;
        pending.clear();
      }
      let seeded: Map<string, Entry[] | { error: string }> | null = null;
      for (const path of need) {
        if (pending.has(path) || (!fresh && cache.has(path))) continue;
        const at: Location = { source: loc.source, path };
        const known = listings.get(at);
        if (known && cache.get(path) !== known) (seeded ??= new Map(cache)).set(path, known);
        pending.add(path);
        const forKey = key;
        listings.fetch(at).then(
          (list) => {
            const shown = cache.get(path);
            if (cacheKey === forKey && !(Array.isArray(shown) && sameListing(shown, list))) cache = new Map(cache).set(path, list);
          },
          (e) => {
            if (cacheKey === forKey && !cache.has(path)) cache = new Map(cache).set(path, { error: errorTitle(toVfsError(e).code) });
          },
        ).finally(() => pending.delete(path));
      }
      if (seeded) cache = seeded;
    });
  });

  const parentCols = $derived(
    parents.map((path, i) => {
      const got = cache.get(path);
      return {
        path,
        child: chain[i + 1] ?? null,
        entries: Array.isArray(got) ? shown(got) : null,
        error: got && !Array.isArray(got) ? got.error : null,
      };
    }),
  );

  // ---- next column: contents of the selected folder ----

  const single = $derived(b.selection.length === 1 ? b.selection[0]! : null);
  const previewPath = $derived(single?.kind === "dir" ? single.path : null);
  let preview = $state.raw<{ path: string; entries: Entry[] | null; error: string | null } | null>(null);

  $effect(() => {
    const path = previewPath;
    const loc = b.location;
    void b.refreshCount;
    if (!path || !loc) {
      preview = null;
      return;
    }
    const at: Location = { source: loc.source, path };
    const known = untrack(() => listings.get(at)) ?? null;
    preview = { path, entries: known, error: null };
    let live = true;
    // Short delay so arrowing through folders doesn't list each one; a
    // cached listing shows meanwhile and is only replaced if it changed.
    const t = setTimeout(() => {
      listings.fetch(at).then(
        (list) => { if (live && !(known && sameListing(known, list))) preview = { path, entries: list, error: null }; },
        (e) => { if (live && !known) preview = { path, entries: [], error: errorTitle(toVfsError(e).code) }; },
      );
    }, PREVIEW_DELAY);
    return () => {
      live = false;
      clearTimeout(t);
    };
  });

  const previewEntries = $derived(preview?.entries ? shown(preview.entries) : null);

  // ---- interaction ----

  const pointer = itemPointer(() => b);
  const cutPrefix = $derived(`${srcKey}|`);

  /** Click in a column other than the current one: move there, select it. */
  function enter(dirPath: string, select: string | null) {
    if (!b.location) return Promise.resolve(false);
    return b.go({ source: b.location.source, path: dirPath }, { select, history: "none" });
  }

  function otherDown(dirPath: string) {
    return (e: MouseEvent, entry: Entry) => {
      if (e.button === 0) void enter(dirPath, entry.path);
    };
  }

  function otherMenu(dirPath: string) {
    return async (e: MouseEvent, entry: Entry) => {
      e.preventDefault();
      e.stopPropagation();
      if (await enter(dirPath, entry.path)) entryMenu(b, e, entry);
    };
  }

  function otherDbl(e: MouseEvent, entry: Entry) {
    void openEntry(b, entry);
  }

  // Keep the newest column in view.
  let strip = $state<HTMLDivElement>();
  $effect(() => {
    void chain.length;
    void previewPath;
    void single;
    if (strip) strip.scrollLeft = strip.scrollWidth;
  });
</script>

<div class="columns" bind:this={strip}>
  {#each parentCols as col (col.path)}
    <div class="col">
      {#if col.entries}
        <Column
          entries={col.entries}
          {srcKey}
          source={b.location?.source ?? null}
          active={false}
          isSelected={(p) => p === col.child}
          reveal={col.child}
          onrowdown={otherDown(col.path)}
          onrowdbl={otherDbl}
          onrowmenu={otherMenu(col.path)}
          onbackground={() => void enter(col.path, null)}
        />
      {:else if col.error}
        <p class="note">{col.error}</p>
      {:else}
        <div class="loading"><span class="spinner"></span></div>
      {/if}
    </div>
  {/each}

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="col current" oncontextmenu={(e) => backgroundMenu(b, e)}>
    <Column
      entries={b.items}
      {srcKey}
      source={b.location?.source ?? null}
      active={ui.windowFocused && tabs.current === b}
      isSelected={(p) => b.selected.has(p)}
      isCut={(p) => clipboard.cutKeys.has(cutPrefix + p) || b.pending.has(p)}
      isFresh={(p) => b.fresh.has(p)}
      gitStatus={(n) => b.git?.entries[n]}
      isLeaving={(p) => b.leaving.has(p)}
      renaming={b.renaming}
      reveal={b.reveal?.path ?? null}
      revealSeq={b.reveal?.seq ?? 0}
      onrowdown={(e, entry) => pointer.down(e, entry)}
      onrowclick={(e, entry) => pointer.click(e, entry, (e.target as HTMLElement).classList.contains("name-text"))}
      onrowdbl={(e, entry) => pointer.dblclick(e, entry)}
      onrowmenu={(e, entry) => entryMenu(b, e, entry)}
      onbackground={() => { pointer.cancel(); b.selectNone(); }}
      onrename={(entry, name) => commitRename(b, entry, name)}
      oncancelrename={() => (b.renaming = null)}
    />
  </div>

  {#if preview}
    {@const dirPath = preview.path}
    <div class="col">
      {#if previewEntries}
        {#if previewEntries.length}
          <Column
            entries={previewEntries}
            {srcKey}
            source={b.location?.source ?? null}
            active={false}
            isSelected={() => false}
            onrowdown={otherDown(dirPath)}
            onrowdbl={otherDbl}
            onrowmenu={otherMenu(dirPath)}
          />
        {:else}
          <p class="note">{preview.error ?? "Empty folder"}</p>
        {/if}
      {:else}
        <div class="loading"><span class="spinner"></span></div>
      {/if}
    </div>
  {:else if single}
    <div class="col info">
      {#if b.location && wantsThumbnail(single, b.location)}
        {#key single.path}<Thumb entry={single} location={b.locationOf(single)} size={128} shadow />{/key}
      {:else}
        <FileIcon type={iconFor(single)} size={128} link={single.isSymlink} />
      {/if}
      <h3 class="selectable">{single.name}</h3>
      <dl>
        <dt>Kind</dt><dd>{kindLabel(single)}</dd>
        {#if single.kind === "file"}<dt>Size</dt><dd>{formatSize(single.size)}</dd>{/if}
        <dt>Modified</dt><dd>{formatDate(single.modified)}</dd>
      </dl>
      <p class="hint">Space for Quick Look</p>
    </div>
  {/if}
</div>

<style>
  .columns {
    height: 100%;
    display: flex;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .col {
    flex: none;
    width: 250px;
    height: 100%;
    border-right: 1px solid var(--separator);
  }
  .loading { height: 100%; display: grid; place-items: center; }
  .note { margin: 14px; color: var(--fg-tertiary); font-size: var(--fs-secondary); text-align: center; }
  .info {
    width: 280px;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 28px 16px;
    gap: 6px;
    overflow-y: auto;
  }
  .info h3 { margin: 8px 0 4px; font-size: var(--fs-ui); text-align: center; overflow-wrap: anywhere; }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 3px 10px; margin: 0; font-size: var(--fs-secondary); }
  dt { color: var(--fg-secondary); text-align: right; }
  dd { margin: 0; }
  .hint { margin-top: 10px; color: var(--fg-tertiary); font-size: var(--fs-small); }
</style>
