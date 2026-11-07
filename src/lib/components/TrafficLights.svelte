<script lang="ts">
  import { ui } from "$lib/stores/ui.svelte";
  import { appWindow } from "$lib/util/window";
</script>

<div class="lights" class:inactive={!ui.windowFocused}>
  <button class="light close" title="Close" aria-label="Close window" tabindex="-1" onclick={() => appWindow.close()}>
    <svg viewBox="0 0 12 12"><path d="M3.6 3.6l4.8 4.8M8.4 3.6L3.6 8.4" /></svg>
  </button>
  <button class="light min" title="Minimize" aria-label="Minimize window" tabindex="-1" onclick={() => appWindow.minimize()}>
    <svg viewBox="0 0 12 12"><path d="M3 6h6" /></svg>
  </button>
  <button class="light zoom" title="Zoom" aria-label="Zoom window" tabindex="-1" onclick={() => appWindow.toggleMaximize()}>
    <svg viewBox="0 0 12 12"><path d="M6 3v6M3 6h6" /></svg>
  </button>
</div>

<style>
  .lights { display: flex; gap: 8px; padding: 0 4px; }
  .light {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    box-shadow: inset 0 0 0 0.5px rgba(0, 0, 0, 0.18);
  }
  .close { background: var(--tl-close); }
  .min { background: var(--tl-min); }
  .zoom { background: var(--tl-zoom); }
  .inactive .light { background: var(--tl-idle); }
  svg { width: 12px; height: 12px; fill: none; stroke: var(--tl-glyph); stroke-width: 1.3; stroke-linecap: round; opacity: 0; }
  .lights:hover svg { opacity: 1; }
  .lights:hover .close { background: var(--tl-close); }
  .lights:hover .min { background: var(--tl-min); }
  .lights:hover .zoom { background: var(--tl-zoom); }
  .light:active { filter: brightness(0.85); }
</style>
