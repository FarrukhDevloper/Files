<!-- Thumbnail (image or video frame) with the type icon as placeholder and
     fallback. See src/lib/thumbs.svelte.ts for when and how it loads. -->
<script lang="ts">
  import { untrack } from "svelte";
  import FileIcon from "./FileIcon.svelte";
  import type { Entry, Location } from "$lib/ipc/types";
  import { hasFailed, isVideoThumb, watchThumbnail, type ThumbHandle } from "$lib/thumbs.svelte";
  import { iconFor } from "$lib/util/filetype";
  import { sourceKey } from "$lib/util/path";

  let { entry, location, size }: { entry: Entry; location: Location; size: number } = $props();

  const key = $derived(`${sourceKey(location.source)}|${location.path}|${entry.size}|${entry.modified}|${size}`);
  const video = $derived(isVideoThumb(entry) && size >= 48);

  let box = $state<HTMLDivElement>();
  let src = $state<string | null>(null);
  let loaded = $state(false);
  let handle: ThumbHandle | null = null;

  $effect(() => {
    const k = key;
    const [e, loc, px] = untrack(() => [entry, location, size] as const);
    src = null;
    loaded = false;
    if (!box || hasFailed(k)) return;
    const h = watchThumbnail(box, e, loc, px, k, (url) => (src = url));
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

  /** Removing src aborts a download still in flight (scrolled away). */
  function abortable(img: HTMLImageElement) {
    return { destroy: () => img.removeAttribute("src") };
  }
</script>

<div class="thumb" class:small={size < 32} bind:this={box} style:width="{size}px" style:height="{size}px">
  {#if !loaded}<FileIcon type={iconFor(entry)} {size} link={entry.isSymlink} />{/if}
  {#if src}
    <img {src} alt="" decoding="async" draggable="false" class:shown={loaded} {onload} {onerror} use:abortable />
  {/if}
  {#if loaded && video}
    <span class="play" aria-hidden="true"><svg viewBox="0 0 10 10"><path d="M3 2l5 3-5 3z" /></svg></span>
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
  img.shown { position: static; opacity: 1; animation: fade-in 0.16s ease-out; }
  @keyframes fade-in { from { opacity: 0; } }
  .small img { border-radius: 2px; box-shadow: 0 0 0 0.5px var(--border-strong); }
  .play {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 26px;
    height: 26px;
    margin: -13px 0 0 -13px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.5);
    box-shadow: 0 0 0 1.5px rgba(255, 255, 255, 0.85);
  }
  .play svg { width: 12px; height: 12px; margin-left: 2px; fill: #fff; }
</style>
