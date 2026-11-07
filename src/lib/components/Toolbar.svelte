<script lang="ts">
  import ChevronLeft from "@lucide/svelte/icons/chevron-left";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import LayoutGrid from "@lucide/svelte/icons/layout-grid";
  import List from "@lucide/svelte/icons/list";
  import Columns from "@lucide/svelte/icons/columns-3";
  import Ellipsis from "@lucide/svelte/icons/ellipsis";
  import ArrowUpDown from "@lucide/svelte/icons/arrow-up-down";
  import Search from "@lucide/svelte/icons/search";
  import CircleX from "@lucide/svelte/icons/circle-x";
  import TrafficLights from "./TrafficLights.svelte";
  import TransfersPopover from "./TransfersPopover.svelte";
  import { getInfo, goToFolder, newFolder, openPreferences } from "$lib/actions";
  import { hosts } from "$lib/stores/hosts.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { toasts } from "$lib/stores/toasts.svelte";
  import { transfers } from "$lib/stores/transfers.svelte";
  import { ui, type MenuItem } from "$lib/stores/ui.svelte";
  import { shortcutLabel } from "$lib/util/platform";

  const b = $derived(tabs.current);
  let search = $state<HTMLInputElement>();

  ui.focusSearch = () => {
    search?.focus();
    search?.select();
  };

  function actionMenu(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const items: MenuItem[] = [
      { label: "New Folder", shortcut: shortcutLabel("Mod+Shift+N"), disabled: !b.location || !!b.error, run: () => void newFolder(b) },
      { label: "Get Info", shortcut: shortcutLabel("Mod+I"), disabled: b.selection.length !== 1, run: () => getInfo(b) },
      { label: "Go to Folder…", shortcut: shortcutLabel("Mod+L"), disabled: !b.location, run: () => goToFolder(b) },
      { label: "New Tab", shortcut: shortcutLabel("Mod+T"), run: () => void tabs.open(b) },
      "separator",
      { label: b.showHidden ? "Hide Hidden Files" : "Show Hidden Files", shortcut: shortcutLabel("Mod+H"), run: () => b.toggleHidden() },
      { label: "Refresh", shortcut: shortcutLabel("Mod+R"), disabled: !b.location, run: () => void b.refresh() },
      "separator",
      { label: "Preferences…", shortcut: shortcutLabel("Mod+,"), run: openPreferences },
      "separator",
      { label: "Reload ~/.ssh/config", run: () => void hosts.reload().catch((err) => toasts.error(err, "Couldn't read ~/.ssh/config")) },
    ];
    ui.menu = { x: r.left, y: r.bottom + 4, items, keyboard: e.detail === 0 };
  }

  function onSearchKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      b.filter = "";
      ui.focusFiles();
    } else if (e.key === "Enter" || e.key === "ArrowDown") {
      e.preventDefault();
      ui.focusFiles();
      if (b.items.length) b.moveTo(0);
    }
  }
</script>

<header class="toolbar" data-tauri-drag-region="deep">
  <TrafficLights />

  <div class="nav">
    <button class="tool" title="Back ({shortcutLabel('Alt+Left')})" aria-label="Back" disabled={!b.canBack} onclick={() => b.back()}>
      <ChevronLeft size={20} strokeWidth={1.8} />
    </button>
    <button class="tool" title="Forward ({shortcutLabel('Alt+Right')})" aria-label="Forward" disabled={!b.canForward} onclick={() => b.forward()}>
      <ChevronRight size={20} strokeWidth={1.8} />
    </button>
  </div>

  <h1 class="title">{b.title}</h1>

  <div class="spacer"></div>

  <div class="seg" role="group" aria-label="View">
    <button class="tool" class:on={b.view === "icons"} title="as Icons ({shortcutLabel('Mod+1')})" aria-label="Icons" onclick={() => b.setView("icons")}>
      <LayoutGrid size={16} strokeWidth={1.8} />
    </button>
    <button class="tool" class:on={b.view === "list"} title="as List ({shortcutLabel('Mod+2')})" aria-label="List" onclick={() => b.setView("list")}>
      <List size={17} strokeWidth={1.8} />
    </button>
    <button class="tool" class:on={b.view === "columns"} title="as Columns ({shortcutLabel('Mod+3')})" aria-label="Columns" onclick={() => b.setView("columns")}>
      <Columns size={16} strokeWidth={1.8} />
    </button>
  </div>

  <button class="tool" title="Actions" aria-label="Actions" aria-haspopup="menu" onclick={actionMenu}>
    <Ellipsis size={18} strokeWidth={1.8} />
  </button>

  <div class="transfers">
    <button
      class="tool"
      class:on={ui.transfersOpen}
      title="Transfers"
      aria-label={transfers.active.length ? `Transfers, ${transfers.active.length} active` : "Transfers"}
      aria-expanded={ui.transfersOpen}
      onclick={() => (ui.transfersOpen = !ui.transfersOpen)}
    >
      <ArrowUpDown size={16} strokeWidth={1.8} />
      {#if transfers.active.length}
        <span class="badge">{transfers.active.length}</span>
        <span class="bar"><span style:width="{Math.round(transfers.progress * 100)}%"></span></span>
      {/if}
    </button>
    {#if ui.transfersOpen}<TransfersPopover />{/if}
  </div>

  <label class="search">
    <Search size={13} strokeWidth={2} />
    <input
      bind:this={search}
      bind:value={b.filter}
      onkeydown={onSearchKey}
      type="search"
      placeholder="Search"
      spellcheck="false"
      autocomplete="off"
      aria-label="Filter this folder"
    />
    {#if b.filter}
      <button class="clear" aria-label="Clear search" onclick={() => (b.filter = "")}><CircleX size={13} /></button>
    {/if}
  </label>
</header>

<style>
  .toolbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: var(--toolbar-h);
    padding: 0 12px 0 14px;
    background: var(--bg-toolbar);
    border-bottom: 1px solid var(--border);
  }
  .nav { display: flex; margin-left: 12px; }
  .nav .tool { min-width: 28px; padding: 0 2px; }
  .title {
    margin: 0 0 0 4px;
    font-size: 14px;
    font-weight: 650;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
    max-width: 40%;
  }
  .spacer { flex: 1; }
  .seg { display: flex; padding: 2px; border-radius: 7px; background: var(--bg-control); }
  .seg .tool { height: 24px; min-width: 32px; border-radius: 5px; }
  .seg .tool.on { background: var(--bg-window); box-shadow: var(--shadow-control); color: var(--fg-primary); }
  .transfers { position: relative; }
  .transfers .tool { position: relative; }
  .badge {
    position: absolute;
    top: 0;
    right: 0;
    min-width: 15px;
    height: 15px;
    padding: 0 4px;
    border-radius: 8px;
    background: var(--accent);
    color: var(--accent-fg);
    font-size: 10px;
    font-weight: 650;
    line-height: 15px;
    text-align: center;
  }
  .bar {
    position: absolute;
    left: 6px;
    right: 6px;
    bottom: 2px;
    height: 2px;
    border-radius: 1px;
    background: var(--progress-track);
    overflow: hidden;
  }
  .bar span { display: block; height: 100%; background: var(--accent); }
  .search {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 190px;
    height: 26px;
    padding: 0 7px;
    border-radius: 7px;
    background: var(--bg-control);
    color: var(--fg-secondary);
    margin-left: 4px;
  }
  .search:focus-within { box-shadow: var(--focus-ring); background: var(--bg-field); }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    padding: 0;
    color: var(--fg-primary);
  }
  .search input::-webkit-search-cancel-button { display: none; }
  .clear { display: grid; place-items: center; color: var(--fg-tertiary); }
  @media (max-width: 860px) { .search { width: 130px; } }
</style>
