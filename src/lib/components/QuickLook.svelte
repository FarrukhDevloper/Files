<script lang="ts">
  import { untrack } from "svelte";
  import X from "@lucide/svelte/icons/x";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Copy from "@lucide/svelte/icons/copy";
  import Play from "@lucide/svelte/icons/play";
  import FileIcon from "./FileIcon.svelte";
  import { openInPlayer, openWithDefaultApp, playerLabel } from "$lib/actions";
  import { api } from "$lib/ipc/api";
  import { thumbUrl } from "$lib/ipc/media";
  import { edits } from "$lib/stores/edits.svelte";
  import type { Entry } from "$lib/ipc/types";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { toasts } from "$lib/stores/toasts.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { toVfsError } from "$lib/util/errors";
  import { rememberFocus, trapTab } from "$lib/util/focus";
  import { formatDate, formatSize } from "$lib/util/format";
  import { iconFor, kindLabel, previewKind } from "$lib/util/filetype";
  import { sourceKey } from "$lib/util/path";

  const MAX_TEXT = 256 * 1024;

  const b = $derived(tabs.current);
  const entry = $derived<Entry | undefined>(
    b.cursor !== null ? b.items[b.index.get(b.cursor) ?? -1] : b.selection[0],
  );
  const kind = $derived(entry ? previewKind(entry) : "none");
  const player = $derived((kind === "video" || kind === "audio") && edits.players.length ? edits.players[0]! : null);
  const poster = $derived(entry && b.location && kind === "video" ? thumbUrl(b.locationOf(entry), 512, entry.modified) ?? undefined : undefined);

  type Loaded =
    | { kind: "loading" }
    | { kind: "media"; url: string }
    | { kind: "text"; text: string; truncated: boolean }
    | { kind: "none"; note?: string };

  let loaded = $state.raw<Loaded>({ kind: "loading" });
  let mediaFailed = $state(false);
  let mediaUrl = $state<string | null>(null);

  // Reload only when the item itself changes, not when the listing is
  // refreshed around it (that would restart a playing video).
  const key = $derived(
    entry && b.location ? `${sourceKey(b.location.source)}|${entry.path}|${entry.size}|${entry.modified}` : null,
  );

  // Load the preview for the current item; stale results are dropped.
  $effect(() => {
    if (!key) {
      loaded = { kind: "none" };
      return;
    }
    const e = untrack(() => entry)!;
    const k = untrack(() => kind);
    const loc = untrack(() => b.locationOf(e));
    mediaFailed = false;
    mediaUrl = null;
    let live = true;
    if (k === "none") {
      loaded = { kind: "none" };
    } else if (k === "text") {
      loaded = { kind: "loading" };
      api.readText(loc, MAX_TEXT).then(
        (t) => live && (loaded = { kind: "text", text: t.text, truncated: t.truncated }),
        (err) => {
          if (!live) return;
          const v = toVfsError(err);
          loaded = { kind: "none", note: v.code === "unsupported" ? undefined : v.message };
        },
      );
    } else {
      loaded = { kind: "loading" };
      api.mediaUrl(loc).then(
        (url) => { if (live) { mediaUrl = url; loaded = { kind: "media", url }; } },
        (err) => live && (loaded = { kind: "none", note: toVfsError(err).message }),
      );
    }
    return () => { live = false; };
  });

  let panel = $state<HTMLDivElement>();
  $effect(() => {
    const restore = rememberFocus();
    untrack(() => panel)?.focus({ preventScroll: true });
    return restore;
  });

  function close() {
    ui.quickLook = false;
  }

  function openExternally() {
    if (entry && b.location) void openWithDefaultApp(entry, b.locationOf(entry));
  }

  function play() {
    if (!entry || !b.location || !player) return;
    close(); // the player takes over; don't keep a second stream running
    void openInPlayer(entry, b.locationOf(entry), player);
  }

  async function copyUrl() {
    if (!mediaUrl) return;
    try {
      await navigator.clipboard.writeText(mediaUrl);
      toasts.info("Stream URL copied", "Paste it into mpv or VLC.");
    } catch {
      toasts.info("Couldn't access the system clipboard");
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="overlay" onclick={(e) => e.target === e.currentTarget && close()}>
  {#if entry}
    <div class="panel" role="dialog" aria-modal="true" aria-label="Quick Look: {entry.name}" tabindex="-1" bind:this={panel} onkeydown={(e) => trapTab(e, panel)}>
      <header>
        <button class="tool" aria-label="Close" title="Close (Space)" onclick={close}><X size={16} /></button>
        <h2>{entry.name}</h2>
        {#if mediaUrl && b.location?.source.kind === "ssh"}
          <button class="tool" title="Copy stream URL" aria-label="Copy stream URL" onclick={copyUrl}><Copy size={15} /></button>
        {/if}
        {#if player}
          <button class="tool" title="Play in {playerLabel(player)}" aria-label="Play in {playerLabel(player)}" onclick={play}><Play size={15} /></button>
        {/if}
        {#if entry.kind === "file"}
          <button class="tool" title="Open with default app" aria-label="Open with default app" onclick={openExternally}><ExternalLink size={15} /></button>
        {/if}
      </header>

      <div class="stage" class:dark={loaded.kind === "media" && kind !== "audio"}>
        {#if loaded.kind === "loading"}
          <span class="spinner"></span>
        {:else if loaded.kind === "media" && !mediaFailed}
          {#if kind === "image"}
            <img src={loaded.url} alt={entry.name} onerror={() => (mediaFailed = true)} />
          {:else if kind === "video"}
            <!-- svelte-ignore a11y_media_has_caption -->
            <video src={loaded.url} {poster} controls autoplay preload="metadata" onerror={() => (mediaFailed = true)}></video>
          {:else}
            <div class="audio">
              <FileIcon type="audio" size={96} />
              <audio src={loaded.url} controls autoplay preload="metadata" onerror={() => (mediaFailed = true)}></audio>
            </div>
          {/if}
        {:else if loaded.kind === "text"}
          <pre class="selectable">{loaded.text}{#if loaded.truncated}<span class="more">
… preview limited to the first {formatSize(MAX_TEXT)}</span>{/if}</pre>
        {:else}
          <div class="fallback">
            <FileIcon type={iconFor(entry)} size={128} link={entry.isSymlink} />
            <h3>{entry.name}</h3>
            <p>{kindLabel(entry)}{entry.kind === "file" ? ` · ${formatSize(entry.size)}` : ""}</p>
            <p>Modified {formatDate(entry.modified)}</p>
            {#if mediaFailed}
              <p class="note">This format can't be played here.{player ? "" : b.location?.source.kind === "ssh" ? " Copy the stream URL to open it in mpv or VLC." : ""}</p>
              {#if player}<button class="btn primary" onclick={play}>Play in {playerLabel(player)}</button>
              {:else if mediaUrl}<button class="btn" onclick={copyUrl}>Copy Stream URL</button>{/if}
            {:else if loaded.kind === "none" && loaded.note}
              <p class="note">{loaded.note}</p>
            {/if}
          </div>
        {/if}
      </div>

      <footer>
        <span>{kindLabel(entry)}</span>
        {#if entry.kind === "file"}<span>{formatSize(entry.size)}</span>{/if}
        <span>{formatDate(entry.modified)}</span>
        <span class="hint">← → to browse · Space to close</span>
      </footer>
    </div>
  {/if}
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 45;
    display: grid;
    place-items: center;
    background: var(--bg-overlay);
    animation: fade 0.12s ease-out;
  }
  .panel:focus-visible { outline: none; }
  .panel {
    width: min(960px, calc(100vw - 80px));
    height: min(720px, calc(100vh - 80px));
    display: flex;
    flex-direction: column;
    border-radius: 12px;
    overflow: hidden;
    background: var(--bg-quicklook);
    box-shadow: var(--shadow-sheet);
    animation: zoom 0.14s ease-out;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 38px;
    padding: 0 6px;
    border-bottom: 1px solid var(--border);
  }
  h2 {
    flex: 1;
    margin: 0;
    font-size: var(--fs-ui);
    font-weight: 600;
    text-align: center;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .stage {
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    overflow: hidden;
    background: var(--bg-window);
  }
  .stage.dark { background: #111; }
  img, video { max-width: 100%; max-height: 100%; object-fit: contain; display: block; }
  video { width: 100%; height: 100%; }
  .audio { display: flex; flex-direction: column; align-items: center; gap: 24px; }
  audio { width: 360px; }
  pre {
    align-self: stretch;
    justify-self: stretch;
    margin: 0;
    padding: 14px 18px;
    overflow: auto;
    font: 12px/1.5 var(--font-mono);
    white-space: pre;
    tab-size: 4;
    cursor: text;
  }
  .more { color: var(--fg-tertiary); }
  .fallback { display: flex; flex-direction: column; align-items: center; gap: 4px; text-align: center; padding: 24px; }
  .fallback h3 { margin: 12px 0 0; font-size: 16px; }
  .fallback p { margin: 0; color: var(--fg-secondary); font-size: var(--fs-secondary); }
  .fallback .note { margin: 10px 0 4px; max-width: 380px; }
  footer {
    flex: none;
    display: flex;
    gap: 14px;
    height: 28px;
    align-items: center;
    padding: 0 14px;
    border-top: 1px solid var(--border);
    font-size: var(--fs-small);
    color: var(--fg-secondary);
  }
  .hint { margin-left: auto; color: var(--fg-tertiary); }
  @keyframes fade { from { opacity: 0; } }
  @keyframes zoom { from { transform: scale(0.96); opacity: 0; } }
</style>
