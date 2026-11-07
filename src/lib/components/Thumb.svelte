<!-- Image thumbnail with the type icon as placeholder and fallback. -->
<script lang="ts">
  import { untrack } from "svelte";
  import FileIcon from "./FileIcon.svelte";
  import type { Entry, Location } from "$lib/ipc/types";
  import { hasFailed, watchThumbnail, type ThumbHandle } from "$lib/thumbs";
  import { iconFor } from "$lib/util/filetype";
  import { sourceKey } from "$lib/util/path";

  let { entry, location, size }: { entry: Entry; location: Location; size: number } = $props();

  const key = $derived(`${sourceKey(location.source)}|${location.path}|${entry.size}|${entry.modified}`);

  let box = $state<HTMLDivElement>();
  let src = $state<string | null>(null);
  let loaded = $state(false);
  let handle: ThumbHandle | null = null;

  $effect(() => {
    const k = key;
    const loc = untrack(() => location);
    src = null;
    loaded = false;
    if (!box || hasFailed(k)) return;
    const h = watchThumbnail(box, loc, k, (url) => (src = url));
    handle = h;
    return () => {
      h.dispose();
      if (handle === h) handle = null;
    };
  });

  function onload() {
    loaded = true;
    handle?.settled(true);
  }

  function onerror() {
    src = null;
    handle?.settled(false);
  }
</script>

<div class="thumb" bind:this={box} style:width="{size}px" style:height="{size}px">
  {#if !loaded}<FileIcon type={iconFor(entry)} {size} link={entry.isSymlink} />{/if}
  {#if src}
    <img {src} alt="" decoding="async" draggable="false" class:shown={loaded} {onload} {onerror} />
  {/if}
</div>

<style>
  .thumb { position: relative; display: grid; place-items: center; flex: none; }
  img {
    position: absolute;
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: 3px;
    box-shadow: 0 0 0 1px var(--border), 0 1px 3px rgba(0, 0, 0, 0.18);
    background: var(--bg-window);
    opacity: 0;
  }
  img.shown { position: static; opacity: 1; }
</style>
