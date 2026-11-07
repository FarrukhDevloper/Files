<!-- The file area: one pane, or two side by side in dual-pane mode (F3).
     Each pane is an independent Browser; the focused one is tabs.current. -->
<script lang="ts">
  import Pane from "./Pane.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { ui } from "$lib/stores/ui.svelte";

  let area = $state<HTMLDivElement>();
  ui.focusFiles = () => area?.querySelector<HTMLElement>(`.content[data-pane="${tabs.focus}"]`)?.focus({ preventScroll: true });
</script>

<div class="area" bind:this={area}>
  <Pane b={tabs.primary} pane={0} dual={!!tabs.split} />
  {#if tabs.split}
    <div class="divider" aria-hidden="true"></div>
    {#key tabs.split}<Pane b={tabs.split} pane={1} dual />{/key}
  {/if}
</div>

<style>
  .area { flex: 1; min-width: 0; min-height: 0; display: flex; }
  .area > :global(.content) { flex: 1 1 0; }
  .divider { flex: none; width: 1px; background: var(--border); }
</style>
