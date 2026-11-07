<script lang="ts">
  import { untrack } from "svelte";
  import Check from "@lucide/svelte/icons/check";
  import { ui, type MenuItem } from "$lib/stores/ui.svelte";
  import { rememberFocus } from "$lib/util/focus";

  const menu = $derived(ui.menu!);
  let el = $state<HTMLDivElement>();
  let pos = $state({ x: 0, y: 0 });
  let active = $state(-1);

  // Keep the menu on screen.
  $effect(() => {
    const m = menu;
    if (!el) return;
    const r = el.getBoundingClientRect();
    pos = {
      x: Math.max(4, Math.min(m.x, window.innerWidth - r.width - 4)),
      y: m.y + r.height > window.innerHeight - 4 ? Math.max(4, m.y - r.height) : m.y,
    };
    active = -1;
    if (m.keyboard) step(1);
  });

  $effect(() => {
    const restore = rememberFocus();
    untrack(() => el)?.focus({ preventScroll: true });
    return restore;
  });

  function run(item: MenuItem) {
    if (item === "separator" || item.disabled) return;
    ui.closeMenu();
    item.run();
  }

  function step(dir: 1 | -1) {
    const items = menu.items;
    for (let n = 0, i = active; n < items.length; n++) {
      i = (i + dir + items.length) % items.length;
      const it = items[i];
      if (it && it !== "separator" && !it.disabled) {
        active = i;
        return;
      }
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Tab") {
      e.preventDefault();
      ui.closeMenu();
      return;
    }
    if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      active = e.key === "Home" ? -1 : menu.items.length;
      step(e.key === "Home" ? 1 : -1);
      return;
    }
    const handled = ["Escape", "ArrowDown", "ArrowUp", "Enter", " "].includes(e.key);
    if (!handled) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") ui.closeMenu();
    else if (e.key === "ArrowDown") step(1);
    else if (e.key === "ArrowUp") step(-1);
    else if (active >= 0) run(menu.items[active]!);
  }

  function onpointerdown(e: PointerEvent) {
    if (el && !el.contains(e.target as Node)) ui.closeMenu();
  }
</script>

<svelte:window {onkeydown} onblur={() => ui.closeMenu()} onresize={() => ui.closeMenu()} />
<svelte:document onpointerdowncapture={onpointerdown} onwheelcapture={() => ui.closeMenu()} />

<div class="menu" role="menu" tabindex="-1" bind:this={el} style:left="{pos.x}px" style:top="{pos.y}px" oncontextmenu={(e) => e.preventDefault()}>
  {#each menu.items as item, i}
    {#if item === "separator"}
      <div class="sep" role="separator"></div>
    {:else}
      <button
        class="item"
        class:active={i === active}
        class:danger={item.danger}
        role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"}
        aria-checked={item.checked === undefined ? undefined : item.checked}
        tabindex="-1"
        disabled={item.disabled}
        onpointerenter={() => (active = item.disabled ? -1 : i)}
        onpointerleave={() => (active = -1)}
        onclick={() => run(item)}
      >
        <span class="check">{#if item.checked}<Check size={12} strokeWidth={2.5} />{/if}</span>
        <span class="label">{item.label}</span>
        {#if item.shortcut}<span class="shortcut">{item.shortcut}</span>{/if}
      </button>
    {/if}
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 50;
    min-width: 200px;
    padding: 5px;
    border-radius: var(--r-popover);
    background: var(--bg-popover);
    box-shadow: var(--shadow-popover);
  }
  .item {
    display: flex;
    align-items: center;
    width: 100%;
    height: 22px;
    padding: 0 10px 0 4px;
    border-radius: 5px;
    text-align: left;
    white-space: nowrap;
  }
  .item:disabled { color: var(--fg-tertiary); }
  .item.active { background: var(--accent); color: var(--accent-fg); }
  .item.danger:not(.active) { color: var(--danger); }
  .check { width: 18px; display: grid; place-items: center; flex: none; }
  .label { flex: 1; }
  .shortcut { margin-left: 24px; color: var(--fg-tertiary); font-size: 12px; }
  .item.active .shortcut { color: inherit; opacity: 0.8; }
  .sep { height: 1px; margin: 5px 10px; background: var(--separator); }
</style>
