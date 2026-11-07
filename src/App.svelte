<script lang="ts">
  import { onMount } from "svelte";
  import FileIconSprite from "$lib/components/FileIconSprite.svelte";
  import Toolbar from "$lib/components/Toolbar.svelte";
  import TabBar from "$lib/components/TabBar.svelte";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import Content from "$lib/components/Content.svelte";
  import PathBar from "$lib/components/PathBar.svelte";
  import ContextMenu from "$lib/components/ContextMenu.svelte";
  import QuickLook from "$lib/components/QuickLook.svelte";
  import Sheets from "$lib/components/Sheets.svelte";
  import Toasts from "$lib/components/Toasts.svelte";
  import DragGhost from "$lib/components/DragGhost.svelte";
  import { dropItems, openStart } from "$lib/actions";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { handleKeydown, handleMouseNav } from "$lib/keyboard";
  import { listenForFileDrops } from "$lib/osDrop";
  import { edits } from "$lib/stores/edits.svelte";
  import { hosts } from "$lib/stores/hosts.svelte";
  import { initThumbs } from "$lib/thumbs.svelte";
  import { prefs } from "$lib/stores/prefs.svelte";
  import { prompts } from "$lib/stores/prompts.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { toasts } from "$lib/stores/toasts.svelte";
  import { transfers } from "$lib/stores/transfers.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { listings } from "$lib/stores/listings";
  import { parentPath, parseSourceKey, sameLocation } from "$lib/util/path";
  import { isTextInput } from "$lib/util/platform";

  dnd.onDrop = (target, items, copy) => void dropItems(target, items, copy);

  onMount(() => {
    // Subscribe before the first listing so no early event is missed.
    void initThumbs();
    Promise.all([prompts.init(), hosts.init(), transfers.init(), edits.init()])
      .then(() => {
        // Like mounted disks: servers come up by themselves (no prompts).
        if (prefs.autoConnect) hosts.connectAllQuietly();
        // ?open=…&view=…&select=… for screenshot runs (src/lib/devhooks.ts).
        if (import.meta.env.DEV) void import("$lib/devhooks").then((m) => m.runDevHooks());
      })
      .catch((e) => toasts.error(e, "Couldn't reach the backend"));
    openStart(tabs.current);
    const offFileDrops = listenForFileDrops();

    const offSettled = transfers.onSettled((job) => {
      if (job.state === "failed") {
        toasts.error(job.error ?? { code: "other", message: "Transfer failed" }, job.op === "move" ? "Move failed" : "Copy failed");
      }
      // Show the result: refresh every tab looking at the destination
      // (or, for a move, at a source folder).
      for (const b of tabs.all) {
        const touched = sameLocation(b.location, job.destDir)
          || (job.op === "move" && job.sources.some((s) => sameLocation(b.location, { source: s.source, path: parentPath(s.path) ?? "/" })));
        if (touched) b.revalidate();
      }
    });

    // Like a mounted disk: a dropped server keeps its folders on screen
    // (the backend reconnects); once it's back they are re-listed quietly.
    const offStatus = hosts.onStatus((host, status, prev) => {
      if (status.state !== "connected" || prev?.state === "connected") return;
      for (const b of tabs.all) {
        const loc = b.location;
        if (loc?.source.kind === "ssh" && loc.source.host === host) b.revalidate();
      }
    });

    return () => {
      offSettled();
      offStatus();
      void offFileDrops.then((off) => off());
    };
  });

  function onmousedown(e: MouseEvent) {
    // Chrome buttons must not take keyboard focus from the file view (Space
    // would re-press them), except inside dialogs.
    const button = (e.target as HTMLElement).closest?.("button");
    if (button && !button.closest("[role=dialog], [role=menu]")) e.preventDefault();
  }

  /** Resting the pointer on any folder (row, sidebar, crumb, card) lists it
   *  ahead of the click. */
  function onmouseover(e: MouseEvent) {
    const el = (e.target as HTMLElement).closest?.<HTMLElement>("[data-drop=dir]");
    const path = el?.dataset.dropPath;
    listings.prefetchSoon(el && path ? { source: parseSourceKey(el.dataset.dropSource ?? "local"), path } : null);
  }

  function oncontextmenu(e: MouseEvent) {
    // No browser/webview menu; text fields and selectable text keep theirs.
    const t = e.target as HTMLElement;
    if (!isTextInput(t) && !t.closest?.(".selectable")) e.preventDefault();
  }
</script>

<svelte:window
  onkeydown={handleKeydown}
  {onmousedown}
  {onmouseover}
  onmouseup={handleMouseNav}
  onfocus={() => (ui.windowFocused = true)}
  onblur={() => (ui.windowFocused = false)}
  {oncontextmenu}
/>

<FileIconSprite />

<div class="app">
  <Toolbar />
  {#if tabs.list.length > 1}<TabBar />{/if}
  <div class="main">
    <Sidebar />
    <Content />
  </div>
  {#if prefs.pathBar}<PathBar />{/if}
</div>

{#if ui.menu}<ContextMenu />{/if}
{#if ui.quickLook}<QuickLook />{/if}
<Sheets />
<Toasts />
{#if dnd.active}<DragGhost />{/if}

<style>
  .app {
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
</style>
