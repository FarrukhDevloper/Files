<!-- One file pane: a Browser's folder, error state or Home page. With dual
     pane on, there are two (see Content.svelte); the focused one is
     tabs.current. -->
<script lang="ts">
  import { untrack } from "svelte";
  import CircleExclamation from "@lucide/svelte/icons/circle-exclamation-point";
  import ListView from "./ListView.svelte";
  import IconView from "./IconView.svelte";
  import ColumnView from "./ColumnView.svelte";
  import HomePage from "./HomePage.svelte";
  import { dnd, dropKey } from "$lib/stores/dnd.svelte";
  import { hosts } from "$lib/stores/hosts.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { errorHint, errorTitle } from "$lib/util/errors";
  import { EASE, reducedMotion } from "$lib/util/motion";
  import type { Browser } from "$lib/stores/browser.svelte";
  import { sourceKey } from "$lib/util/path";

  let { b, pane, dual }: { b: Browser; pane: 0 | 1; dual: boolean } = $props();

  const focused = $derived(tabs.focus === pane);
  /** Dual pane: the whole pane is a drop target for its folder. */
  const dropTarget = $derived(dual && b.location ? b.location : null);
  const dropHere = $derived(!!dropTarget && dnd.target?.key === dropKey(dropTarget));

  // Only a folder that takes a while gets an indicator (cached ones are instant).
  let slow = $state(false);
  $effect(() => {
    if (!b.loading) {
      slow = false;
      return;
    }
    const t = setTimeout(() => (slow = true), 200);
    return () => clearTimeout(t);
  });

  // Folder transitions: a new listing slides in from the side it came from
  // (deeper = from the right, back/up = from the left), else just fades.
  // Only on navigation within a tab; never for background refreshes.
  let page = $state<HTMLDivElement>();
  let lastTab: Browser | null = null;
  let lastListing = -1;
  $effect(() => {
    const tab = b;
    const id = b.listingId;
    const el = page;
    if (!el) return;
    untrack(() => {
      const navigated = tab === lastTab && id !== lastListing;
      lastTab = tab;
      lastListing = id;
      // Column view moves its own columns; sliding the page too would jar.
      if (!navigated || reducedMotion() || (tab.view === "columns" && !tab.home)) return;
      const dx = tab.navDir * 18;
      el.animate(
        dx
          ? [{ opacity: 0.25, transform: `translateX(${dx}px)` }, { opacity: 1, transform: "none" }]
          : [{ opacity: 0.4 }, { opacity: 1 }],
        { duration: dx ? 200 : 140, easing: EASE },
      );
    });
  });


  async function retry() {
    const tab = b;
    const loc = tab.location;
    if (!loc) return;
    const token = tab.claimNav();
    if (loc.source.kind === "ssh" && tab.error?.code !== "host_key") {
      try { await hosts.connect(loc.source.host); } catch { /* go() reports */ }
      if (!tab.isCurrentNav(token)) return;
    }
    tab.error = null;
    await tab.go(loc, { history: "none" });
  }
</script>

<!-- Focusable so keyboard users can Tab into the file view; plain keys act
     on it (src/lib/keyboard.ts). The listboxes inside carry the roles. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<section
  class="content"
  class:dual
  class:focused
  tabindex="0"
  aria-label={dual ? (pane === 0 ? "Left pane" : "Right pane") : "Files"}
  data-pane={pane}
  data-drop={dropTarget ? "dir" : undefined}
  data-drop-source={dropTarget ? sourceKey(dropTarget.source) : undefined}
  data-drop-path={dropTarget?.path}
  onfocusin={() => (tabs.focus = pane)}
  onmousedowncapture={() => (tabs.focus = pane)}
>
  <div class="page" bind:this={page}>
  {#if b.home}
    <HomePage {b} />
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
  </div>
  {#if slow}<div class="loading-bar" role="progressbar" aria-label="Loading folder"><span></span></div>{/if}
  {#if b.location && (dual ? dropHere : dnd.externalOnView)}
    <div class="os-drop"><span>{dnd.external || dnd.copy ? "Copy" : "Move"} to “{b.title}”</span></div>
  {/if}
</section>

<style>
  .content:focus-visible { outline-offset: -3px; border-radius: 4px; }
  /* The focused pane gets a hairline accent on top. */
  .content.dual::before {
    content: "";
    position: absolute;
    inset: 0 0 auto;
    z-index: 4;
    height: 2px;
    background: var(--accent);
    opacity: 0;
    transition: opacity var(--dur-fast) var(--ease-out);
    pointer-events: none;
  }
  .content.dual.focused::before { opacity: 1; }
  /* In dual pane the accent line marks focus; no ring around the pane. */
  .content.dual:focus-visible { outline: none; }
  .content {
    position: relative;
    flex: 1;
    min-width: 0;
    min-height: 0;
    background: var(--bg-window);
  }
  .page { position: relative; height: 100%; }
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
  .loading-bar {
    position: absolute;
    inset: 0 0 auto;
    z-index: 3;
    height: 2px;
    overflow: hidden;
    pointer-events: none;
  }
  .loading-bar span {
    display: block;
    width: 30%;
    height: 100%;
    border-radius: 1px;
    background: var(--accent);
    animation: sweep 1s ease-in-out infinite;
  }
  @keyframes sweep {
    from { transform: translateX(-100%); }
    to { transform: translateX(340%); }
  }
  .empty {
    position: absolute;
    inset: 40% 0 auto;
    text-align: center;
    color: var(--fg-tertiary);
    pointer-events: none;
  }
</style>
