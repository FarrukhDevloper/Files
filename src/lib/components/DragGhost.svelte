<script lang="ts">
  import FileIcon from "./FileIcon.svelte";
  import { dnd } from "$lib/stores/dnd.svelte";

  const p = $derived(dnd.payload!);
</script>

<div class="ghost" style:transform="translate({dnd.x + 10}px, {dnd.y + 8}px)" aria-hidden="true">
  <FileIcon type={p.icon} size={32} />
  <span class="label">{p.label}</span>
  {#if p.items.length > 1}<span class="count">{p.items.length}</span>{/if}
  {#if dnd.target && dnd.copy}<span class="plus">+</span>{/if}
</div>

<style>
  .ghost {
    position: fixed;
    left: 0;
    top: 0;
    z-index: 70;
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: 260px;
    padding: 4px 8px 4px 4px;
    border-radius: 8px;
    background: var(--bg-popover);
    box-shadow: var(--shadow-popover);
    opacity: 0.92;
    pointer-events: none;
    will-change: transform;
  }
  .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
  .count, .plus {
    position: absolute;
    top: -6px;
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 9px;
    color: #fff;
    font-size: 11px;
    font-weight: 700;
    line-height: 18px;
    text-align: center;
  }
  .count { left: 22px; background: var(--danger); }
  .plus { right: -6px; background: var(--dot-on); font-size: 14px; }
</style>
