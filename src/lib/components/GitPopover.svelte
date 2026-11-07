<!-- Repository details for the address bar's branch chip: branch and
     upstream, remote with a link to its web page, and recent commits. -->
<script lang="ts">
  import { untrack } from "svelte";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Terminal from "@lucide/svelte/icons/terminal";
  import FolderGit from "@lucide/svelte/icons/folder-git-2";
  import { openTerminalAt } from "$lib/actions";
  import { api } from "$lib/ipc/api";
  import type { GitCommit, GitInfo } from "$lib/ipc/types";
  import type { Browser } from "$lib/stores/browser.svelte";
  import { toasts } from "$lib/stores/toasts.svelte";
  import { formatDate, plural } from "$lib/util/format";
  import { rememberFocus, trapTab } from "$lib/util/focus";
  import { branchLabel, hostingName } from "$lib/util/git";
  import { baseName } from "$lib/util/path";
  import { openUrl } from "$lib/util/window";

  let { b, git, onclose }: { b: Browser; git: GitInfo; onclose: () => void } = $props();

  const LOG_LIMIT = 15;

  let el = $state<HTMLDivElement>();
  let commits = $state.raw<GitCommit[] | null>(null);
  let failed = $state(false);

  // History of the shown folder, loaded when the popover opens.
  $effect(() => {
    const loc = untrack(() => b.location);
    if (!loc) return;
    let live = true;
    api.gitLog(loc, LOG_LIMIT).then(
      (list) => { if (live) commits = list; },
      () => { if (live) failed = true; },
    );
    return () => { live = false; };
  });

  $effect(() => {
    const restore = rememberFocus();
    untrack(() => el)?.focus({ preventScroll: true });
    return restore;
  });

  const atRoot = $derived(b.location?.path === git.root);

  function openRoot() {
    const loc = b.location;
    if (loc) void b.go({ source: loc.source, path: git.root });
    onclose();
  }

  function onpointerdown(e: PointerEvent) {
    const t = e.target as HTMLElement;
    if (el && !el.contains(t) && !t.closest(".git-chip")) onclose();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    } else if (el?.contains(document.activeElement)) {
      trapTab(e, el);
    }
  }
</script>

<svelte:document onpointerdowncapture={onpointerdown} />
<svelte:window onkeydowncapture={onkeydown} />

<div class="popover" bind:this={el} role="dialog" aria-label="Git repository" tabindex="-1">
  <div class="head">
    <span class="glyph"><FolderGit size={20} strokeWidth={1.7} /></span>
    <div class="title">
      <h2>{baseName(git.root) || git.root}</h2>
      <p class="branch"><GitBranch size={13} strokeWidth={2} /> {branchLabel(git)}{git.upstream ? ` → ${git.upstream}` : ""}</p>
    </div>
  </div>

  <dl>
    {#if git.upstream}
      <dt>Sync</dt>
      <dd>
        {#if git.ahead || git.behind}
          {git.ahead ? `${git.ahead} to push` : ""}{git.ahead && git.behind ? " · " : ""}{git.behind ? `${git.behind} to pull` : ""}
        {:else}
          Up to date
        {/if}
      </dd>
    {/if}
    <dt>Changes</dt>
    <dd>{git.changed ? plural(git.changed, "changed file") : "Clean"}</dd>
    {#if git.remoteUrl}<dt>Remote</dt><dd class="mono selectable" title={git.remoteUrl}>{git.remoteUrl}</dd>{/if}
  </dl>

  <div class="buttons">
    {#if git.webUrl}
      {@const url = git.webUrl}
      <button class="btn primary" onclick={() => void openUrl(url).catch((e) => toasts.error(e, "Couldn't open the browser"))}>
        <ExternalLink size={14} /> Open on {hostingName(url)}
      </button>
    {/if}
    {#if !atRoot}<button class="btn" onclick={openRoot}>Repository Root</button>{/if}
    <button class="btn" title="Open in Terminal" aria-label="Open in Terminal" onclick={() => b.location && void openTerminalAt(b.location)}>
      <Terminal size={14} />
    </button>
  </div>

  <h3>Recent commits{atRoot ? "" : " here"}</h3>
  {#if commits === null && !failed}
    <div class="loading"><span class="spinner"></span></div>
  {:else if failed || !commits?.length}
    <p class="empty">{failed ? "Couldn't read the history." : "No commits yet."}</p>
  {:else}
    <ul>
      {#each commits as c (c.hash)}
        <li>
          <div class="subject">
            <span>{c.subject}</span>
            {#each c.refs as r (r)}<span class="ref" class:tag={r.startsWith("tag:")}>{r.replace(/^tag: /, "")}</span>{/each}
          </div>
          <div class="meta"><span class="mono selectable">{c.shortHash}</span> · {c.author} · {formatDate(c.date)}</div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .popover {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    z-index: 30;
    width: 400px;
    max-height: min(560px, calc(100vh - 120px));
    display: flex;
    flex-direction: column;
    padding: 14px 14px 8px;
    gap: 10px;
    border-radius: var(--r-popover);
    background: var(--bg-popover);
    box-shadow: var(--shadow-popover);
    cursor: default;
    transform-origin: top right;
    animation: pop var(--dur-fast) var(--ease-out);
  }
  .popover:focus-visible { outline: none; }
  @keyframes pop { from { transform: translateY(-4px) scale(0.97); opacity: 0; } }
  .head { display: flex; align-items: center; gap: 10px; }
  .glyph {
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 9px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .title { min-width: 0; }
  h2 { margin: 0; font-size: var(--fs-ui); font-weight: 650; }
  .branch { display: flex; align-items: center; gap: 4px; margin: 2px 0 0; color: var(--fg-secondary); font-size: var(--fs-secondary); }
  dl { display: grid; grid-template-columns: 72px 1fr; gap: 4px 10px; margin: 0; font-size: var(--fs-secondary); }
  dt { color: var(--fg-secondary); }
  dd { margin: 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .mono { font-family: var(--font-mono); font-size: var(--fs-small); }
  .buttons { display: flex; gap: 8px; }
  .buttons .btn { min-width: 0; }
  h3 { margin: 4px 0 0; font-size: var(--fs-small); font-weight: 600; color: var(--fg-secondary); }
  .loading { display: grid; place-items: center; padding: 16px; }
  .empty { margin: 0 0 8px; color: var(--fg-tertiary); font-size: var(--fs-secondary); }
  ul { list-style: none; margin: 0 -6px; padding: 0; overflow-y: auto; }
  li { padding: 6px; border-radius: 6px; }
  li + li { border-top: 1px solid var(--separator); }
  .subject { display: flex; align-items: center; gap: 6px; min-width: 0; }
  .subject > span:first-child { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
  .ref {
    flex: none;
    padding: 1px 6px;
    border-radius: 9px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 11px;
    font-weight: 600;
  }
  .ref.tag { background: var(--bg-control); color: var(--fg-secondary); }
  .meta { margin-top: 2px; color: var(--fg-tertiary); font-size: var(--fs-small); }
</style>
