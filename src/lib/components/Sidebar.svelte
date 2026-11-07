<script lang="ts">
  import Server from "@lucide/svelte/icons/server";
  import Plus from "@lucide/svelte/icons/plus";
  import { placeIcons, placeLabel, StartIcon } from "./places";
  import { addServer, hostAddress, hostMenu, openHost, openPlace } from "$lib/actions";
  import type { Place, HostInfo } from "$lib/ipc/types";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { hosts } from "$lib/stores/hosts.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { cssZoom } from "$lib/util/zoom";

  const b = $derived(tabs.current);

  function placeActive(p: Place): boolean {
    const loc = b.location;
    return loc?.source.kind === "local" && loc.path === p.path;
  }

  function hostActive(alias: string): boolean {
    const loc = b.location;
    return loc?.source.kind === "ssh" && loc.source.host === alias;
  }

  function hostTitle(h: HostInfo): string {
    const target = hostAddress(h);
    if (h.status.state === "failed") return `${target}\n${h.status.error.message}`;
    return `${target} — ${h.status.state}`;
  }

  /** ↑/↓/Home/End move focus between sidebar items. */
  function roving(e: KeyboardEvent) {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    const items = [...(e.currentTarget as HTMLElement).querySelectorAll<HTMLElement>(".item")];
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (i < 0) return;
    e.preventDefault();
    e.stopPropagation();
    const next = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1 : i + (e.key === "ArrowDown" ? 1 : -1);
    items[Math.max(0, Math.min(items.length - 1, next))]?.focus();
  }

  // Resize handle: pointer capture, no global listeners left behind.
  function startResize(e: PointerEvent) {
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const startW = prefs.sidebarWidth;
    const z = cssZoom();
    const move = (ev: PointerEvent) => {
      prefs.sidebarWidth = Math.round(Math.min(380, Math.max(170, startW + (ev.clientX - startX) / z)));
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<nav class="sidebar" style:width="{prefs.sidebarWidth}px" aria-label="Sidebar" onkeydown={roving}>
  <div class="scroll">
    <button class="item start" class:active={b.home} onclick={() => b.showHome()} title="Home: this computer and your servers">
      <span class="icon"><StartIcon size={18} strokeWidth={1.8} /></span>
      <span class="label">Home</span>
    </button>

    {#if prefs.sections.favourites && hosts.favourites.length}
      <h3>Favourites</h3>
      {#each hosts.favourites as p (p.path)}
        {@const Icon = placeIcons[p.kind]}
        <button
          class="item"
          class:active={placeActive(p)}
          class:drop={dnd.target?.key === `local|${p.path}`}
          data-drop="dir"
          data-drop-source="local"
          data-drop-path={p.path}
          onclick={() => openPlace(b, p)}
          title={p.path}
        >
          <span class="icon"><Icon size={18} strokeWidth={1.8} /></span>
          <span class="label">{placeLabel(p)}</span>
        </button>
      {/each}
    {/if}

    {#if prefs.sections.locations && hosts.locations.length}
      <h3>Locations</h3>
      {#each hosts.locations as p (p.path)}
        {@const Icon = placeIcons[p.kind]}
        <button
          class="item"
          class:active={placeActive(p)}
          class:drop={dnd.target?.key === `local|${p.path}`}
          data-drop="dir"
          data-drop-source="local"
          data-drop-path={p.path}
          onclick={() => openPlace(b, p)}
          title={p.path}
        >
          <span class="icon"><Icon size={18} strokeWidth={1.8} /></span>
          <span class="label">{placeLabel(p)}</span>
        </button>
      {/each}
    {/if}

    {#if prefs.sections.servers}
    <div class="head">
      <h3>Servers</h3>
      <button class="add" title="Add Server…" aria-label="Add server" onclick={addServer}><Plus size={14} strokeWidth={2.2} /></button>
    </div>
    {#each hosts.list as h (h.alias)}
      <button
        class="item"
        class:active={hostActive(h.alias)}
        class:drop={dnd.target?.key === `host:${h.alias}`}
        data-drop="host"
        data-drop-host={h.alias}
        onclick={() => openHost(b, h.alias)}
        oncontextmenu={(e) => hostMenu(b, e, h.alias)}
        title={hostTitle(h)}
      >
        <span class="icon"><Server size={18} strokeWidth={1.8} /></span>
        <span class="label">{h.alias}</span>
        <span class="dot {h.status.state}" role="img" aria-label={h.status.state}></span>
      </button>
    {:else}
      <button class="empty" onclick={addServer}>No servers yet. Add one…</button>
    {/each}
    {/if}
  </div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="resize" onpointerdown={startResize} ondblclick={() => (prefs.sidebarWidth = 230)}></div>
</nav>

<style>
  .sidebar {
    position: relative;
    flex: none;
    display: flex;
    flex-direction: column;
    background: var(--bg-sidebar);
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .scroll { flex: 1; overflow-y: auto; padding: 6px 10px 12px; }
  h3 {
    margin: 12px 0 2px 8px;
    font-size: var(--fs-small);
    font-weight: 600;
    color: var(--fg-tertiary);
  }
  .start { margin-bottom: 2px; }
  .head { display: flex; align-items: center; justify-content: space-between; }
  .head h3 { flex: 1; }
  .add {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    margin: 10px 4px 0 0;
    border-radius: 5px;
    color: var(--fg-tertiary);
    opacity: 0;
  }
  .sidebar:hover .add, .add:focus-visible { opacity: 1; }
  .add:hover { background: var(--bg-hover); color: var(--fg-primary); }
  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: var(--side-row-h);
    padding: 0 8px;
    border-radius: 5px;
    text-align: left;
    color: var(--fg-primary);
  }
  .item:hover { background: var(--bg-hover); }
  .item.active { background: var(--bg-side-sel); }
  .item.drop { background: var(--accent); color: var(--accent-fg); }
  .item.drop .icon { color: inherit; }
  .icon { display: grid; place-items: center; color: var(--fg-side-icon); }
  .label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .empty { margin: 4px 8px; padding: 0; color: var(--fg-tertiary); font-size: var(--fs-small); text-align: left; }
  .empty:hover { color: var(--accent); }
  .resize {
    position: absolute;
    top: 0;
    right: -3px;
    width: 6px;
    height: 100%;
    cursor: col-resize;
    z-index: 2;
  }
</style>
