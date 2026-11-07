<!-- Finder-style sheet: drops down from under the toolbar over a dimmed
     window. Escape cancels; the content provides its own buttons. -->
<script lang="ts">
  import type { Snippet } from "svelte";
  import { rememberFocus, trapTab } from "$lib/util/focus";

  let { children, oncancel, label, width = 420 }: {
    children: Snippet;
    oncancel: () => void;
    label: string;
    width?: number;
  } = $props();

  let el = $state<HTMLDivElement>();

  $effect(() => {
    const restore = rememberFocus();
    // Focus the first field, else the default button, else the sheet.
    const target = el?.querySelector<HTMLElement>("input:not([type=radio]), input[type=radio]:checked, select, textarea, [data-default]") ?? el;
    target?.focus();
    return restore;
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      oncancel();
    } else {
      trapTab(e, el);
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="backdrop">
  <div class="sheet" role="dialog" aria-modal="true" aria-label={label} tabindex="-1" bind:this={el} style:width="{width}px">
    {@render children()}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: var(--toolbar-h) 0 0;
    z-index: 40;
    display: flex;
    justify-content: center;
    background: var(--bg-overlay);
    animation: fade var(--dur-fast) var(--ease-out);
  }
  .sheet {
    align-self: flex-start;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - var(--toolbar-h) - 24px);
    overflow: auto;
    padding: 20px;
    border-radius: 0 0 var(--r-popover) var(--r-popover);
    background: var(--bg-sheet);
    box-shadow: var(--shadow-sheet);
    transform-origin: top center;
    animation: drop var(--dur) var(--ease-out);
  }
  @keyframes drop { from { transform: translateY(-14px) scale(0.98); opacity: 0; } }
  @keyframes fade { from { opacity: 0; } }
</style>
