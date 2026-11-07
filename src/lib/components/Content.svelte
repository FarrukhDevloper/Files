<script lang="ts">
  import CircleExclamation from "@lucide/svelte/icons/circle-exclamation-point";
  import ListView from "./ListView.svelte";
  import IconView from "./IconView.svelte";
  import ColumnView from "./ColumnView.svelte";
  import HomePage from "./HomePage.svelte";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { hosts } from "$lib/stores/hosts.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { errorHint, errorTitle } from "$lib/util/errors";

  const b = $derived(tabs.current);

  let section = $state<HTMLElement>();
  ui.focusFiles = () => section?.focus({ preventScroll: true });

  async function retry() {
    const loc = b.location;
    if (!loc) return;
    if (loc.source.kind === "ssh" && b.error?.code !== "host_key") {
      try { await hosts.connect(loc.source.host); } catch { /* go() reports */ }
    }
    b.error = null;
    await b.go(loc, { history: "none" });
  }
</script>

<!-- Focusable so keyboard users can Tab into the file view; plain keys act
     on it (src/lib/keyboard.ts). The listboxes inside carry the roles. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<section class="content" tabindex="0" aria-label="Files" bind:this={section}>
  {#if b.home}
    <HomePage />
  {:else if b.error}
    <div class="state">
      <CircleExclamation size={36} strokeWidth={1.5} />
      <h2>{errorTitle(b.error.code)}</h2>
      <p class="selectable">{errorHint(b.error)}</p>
      {#if b.error.code !== "host_key"}
        <button class="btn" onclick={retry}>{b.error.code === "not_connected" ? "Connect" : "Try Again"}</button>
      {/if}
    </div>
  {:else if !b.location}
    <div class="state"><span class="spinner"></span></div>
  {:else}
    {#key b}
      {#if b.view === "icons"}
        <IconView {b} />
      {:else if b.view === "columns"}
        <ColumnView {b} />
      {:else}
        <ListView {b} />
      {/if}
    {/key}
    {#if b.items.length === 0 && !b.loading && b.view !== "columns"}
      <div class="empty">
        {b.filter ? `No items match “${b.filter}”` : "This folder is empty"}
      </div>
    {/if}
  {/if}
  {#if dnd.externalOnView && b.location}
    <div class="os-drop"><span>Copy to “{b.title}”</span></div>
  {/if}
</section>

<style>
  .content:focus-visible { outline-offset: -3px; border-radius: 4px; }
  .content {
    position: relative;
    flex: 1;
    min-width: 0;
    min-height: 0;
    background: var(--bg-window);
  }
  .state {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 24px;
    color: var(--fg-secondary);
    text-align: center;
  }
  .state h2 { margin: 4px 0 0; font-size: var(--fs-title); color: var(--fg-primary); }
  .state p { margin: 0 0 8px; max-width: 420px; white-space: pre-line; font-size: var(--fs-secondary); }
  .os-drop {
    position: absolute;
    inset: 4px;
    display: flex;
    align-items: flex-end;
    justify-content: center;
    padding-bottom: 16px;
    border-radius: 8px;
    box-shadow: inset 0 0 0 3px var(--accent);
    background: var(--accent-soft);
    pointer-events: none;
  }
  .os-drop span {
    padding: 4px 10px;
    border-radius: 6px;
    background: var(--accent);
    color: var(--accent-fg);
    font-weight: 550;
  }
  .empty {
    position: absolute;
    inset: 40% 0 auto;
    text-align: center;
    color: var(--fg-tertiary);
    pointer-events: none;
  }
</style>
