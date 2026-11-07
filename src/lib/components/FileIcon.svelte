<script lang="ts">
  import type { IconType } from "$lib/util/filetype";

  let { type, size = 16, link = false }: { type: IconType; size?: number; link?: boolean } = $props();
</script>

<svg class="fi" width={size} height={size} viewBox="0 0 64 64" aria-hidden="true">
  <use href="#fi-{type}" />
  {#if link}<use href="#fi-link" />{/if}
</svg>

<style>
  .fi { flex: none; display: block; overflow: visible; }
</style>
