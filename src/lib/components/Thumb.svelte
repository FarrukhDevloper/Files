<!-- Thumbnail (image or video frame) in one fixed square frame, the same box
     as the file icon it replaces, whatever the image's aspect: "Fill" crops
     it to the square (uniform grid), "Fit" shows it whole on a neutral
     backdrop (Preferences → Thumbnails). The type icon is the placeholder
     and the fallback. See src/lib/thumbs.svelte.ts for loading. -->
<script lang="ts">
  import { untrack } from "svelte";
  import FileIcon from "./FileIcon.svelte";
  import type { Entry, Location } from "$lib/ipc/types";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { hasFailed, isVideoThumb, watchThumbnail, type ThumbHandle } from "$lib/thumbs.svelte";
  import { iconFor } from "$lib/util/filetype";
  import { sourceKey } from "$lib/util/path";

  let { entry, location, size, shadow = false }: {
    entry: Entry;
    location: Location;
    /** Frame edge in CSS px (the icon size of the view). */
    size: number;
    /** Soft drop shadow (icon view only). */
    shadow?: boolean;
  } = $props();

  const key = $derived(`${sourceKey(location.source)}|${location.path}|${entry.size}|${entry.modified}|${size}`);
  const video = $derived(isVideoThumb(entry) && size >= 48);
  const fill = $derived(prefs.thumbFit === "fill");
  const radius = $derived(size < 32 ? 3 : size >= 120 ? 9 : 6);

  let box = $state<HTMLDivElement>();
  let src = $state<string | null>(null);
  let loaded = $state(false);
  let handle: ThumbHandle | null = null;

  $effect(() => {
    const k = key;
    const [e, loc, px, crop] = untrack(() => [entry, location, size, fill] as const);
    src = null;
    loaded = false;
    if (!box || hasFailed(k)) return;
    // Cropping to a square needs more pixels on the long side; ask a size up
    // (rows stay small: thumbs.svelte.ts caps them).
    const request = crop && px >= 32 ? Math.round(px * 1.5) : px;
    const h = watchThumbnail(box, e, loc, request, k, (url) => (src = url));
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

<div
  class="thumb"
  class:fit={!fill}
  class:shadow
  class:loaded
  bind:this={box}
  style:width="{size}px"
  style:height="{size}px"
  style:--r="{radius}px"
>
  {#if !loaded}<FileIcon type={iconFor(entry)} {size} link={entry.isSymlink} />{/if}
  {#if src}
    <img {src} alt="" decoding="async" draggable="false" class:shown={loaded} {onload} {onerror} use:abortable />
  {/if}
  {#if loaded && video}
    <span class="play" aria-hidden="true"><svg viewBox="0 0 10 10"><path d="M3 2l5 3-5 3z" /></svg></span>
  {/if}
</div>

<style>
  .thumb {
    position: relative;
    display: grid;
    place-items: center;
    flex: none;
    border-radius: var(--r);
  }
  /* The frame never changes size: the image fills it exactly. */
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: center;
    border-radius: var(--r);
    opacity: 0;
  }
  .fit img { object-fit: contain; background: var(--thumb-backdrop); }
  img.shown { opacity: 1; animation: fade-in 0.16s ease-out; }
  @keyframes fade-in { from { opacity: 0; } }
  .shadow.loaded img { box-shadow: 0 1px 4px rgba(0, 0, 0, 0.22); }
  /* Hairline on top of the image so white pictures keep an edge. */
  .loaded::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: var(--r);
    box-shadow: inset 0 0 0 1px var(--thumb-edge);
    pointer-events: none;
  }
  .play {
    position: absolute;
    left: 50%;
    top: 50%;
    z-index: 1;
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
