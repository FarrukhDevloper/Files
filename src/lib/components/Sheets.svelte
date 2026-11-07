<script lang="ts">
  import KeyRound from "@lucide/svelte/icons/key-round";
  import Lock from "@lucide/svelte/icons/lock";
  import TriangleExclamation from "@lucide/svelte/icons/triangle-exclamation-point";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import Server from "@lucide/svelte/icons/server";
  import Sheet from "./Sheet.svelte";
  import FileIcon from "./FileIcon.svelte";
  import { goToPath } from "$lib/actions";
  import { prefs, type SidebarSections } from "$lib/stores/prefs.svelte";
  import { prompts } from "$lib/stores/prompts.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import type { Entry } from "$lib/ipc/types";
  import { ui, type ConflictAction } from "$lib/stores/ui.svelte";
  import { formatDate, formatDateLong, formatMode, formatSize } from "$lib/util/format";
  import { iconFor, kindLabel } from "$lib/util/filetype";
  import { parentPath, sourceLabel } from "$lib/util/path";

  let secret = $state("");
  let gotoValue = $state("");
  let busy = $state(false);

  // Reset per-sheet fields whenever a new one opens.
  $effect(() => {
    void prompts.current;
    secret = "";
  });
  $effect(() => {
    const s = ui.sheet;
    if (s?.kind === "goto") gotoValue = s.location.path;
  });

  function answer(accept: boolean) {
    const p = prompts.current;
    if (!p) return;
    void prompts.respond(accept, p.kind === "hostKey" ? null : secret);
    secret = "";
  }

  function confirm(ok: boolean) {
    const s = ui.sheet;
    if (s?.kind !== "confirm") return;
    ui.sheet = null;
    s.resolve(ok);
  }

  let applyAll = $state(false);

  function setHidden(show: boolean) {
    prefs.showHidden = show;
    for (const t of tabs.list) t.setHidden(show);
  }

  function section(key: keyof SidebarSections, on: boolean) {
    prefs.sections = { ...prefs.sections, [key]: on };
  }

  function conflict(action: ConflictAction) {
    const s = ui.sheet;
    if (s?.kind !== "conflict") return;
    const all = applyAll && s.remaining > 1;
    applyAll = false;
    ui.sheet = null;
    s.resolve({ action, all });
  }

  function describe(e: Entry): string {
    const what = e.kind === "dir" ? "Folder" : formatSize(e.size);
    return `${what}, modified ${formatDate(e.modified)}`;
  }

  async function submitGoto(e: SubmitEvent) {
    e.preventDefault();
    const s = ui.sheet;
    if (s?.kind !== "goto" || busy) return;
    busy = true;
    const ok = await goToPath(tabs.current, s.location.source, gotoValue);
    busy = false;
    if (ok && ui.sheet === s) ui.sheet = null;
  }
</script>

{#if prompts.current}
  {@const p = prompts.current}
  {#key p.id}
  <Sheet label="Connection prompt" oncancel={() => answer(false)}>
    <form class="body" onsubmit={(e) => { e.preventDefault(); answer(true); }}>
      <div class="glyph">
        {#if p.kind === "password"}<Lock size={30} strokeWidth={1.6} />{:else}<KeyRound size={30} strokeWidth={1.6} />{/if}
      </div>
      <div class="text">
        {#if p.kind === "hostKey"}
          <h2>The authenticity of host “{p.host}” can't be established.</h2>
          <p>Its {p.algorithm} key fingerprint is:</p>
          <code class="selectable">{p.fingerprint}</code>
          <p class="note">Continue only if this matches the server's real key. It will be added to ~/.ssh/known_hosts.</p>
        {:else if p.kind === "password"}
          <h2>Enter the password for “{p.user}@{p.host}”.</h2>
          <input class="field" type="password" bind:value={secret} autocomplete="off" aria-label="Password" />
        {:else}
          <h2>Enter the passphrase for the key used by “{p.host}”.</h2>
          <p class="selectable path">{p.keyPath}</p>
          <input class="field" type="password" bind:value={secret} autocomplete="off" aria-label="Passphrase" />
        {/if}
        <div class="buttons">
          <button type="button" class="btn" onclick={() => answer(false)}>Cancel</button>
          <button type="submit" class="btn primary" data-default>{p.kind === "hostKey" ? "Trust & Connect" : "Connect"}</button>
        </div>
      </div>
    </form>
  </Sheet>
  {/key}
{:else if ui.sheet?.kind === "confirm"}
  {@const s = ui.sheet}
  <Sheet label={s.title} oncancel={() => confirm(false)}>
    <form class="body" onsubmit={(e) => { e.preventDefault(); confirm(true); }}>
      <div class="glyph" class:warn={s.danger}><TriangleExclamation size={30} strokeWidth={1.6} /></div>
      <div class="text">
        <h2>{s.title}</h2>
        <p>{s.message}</p>
        <div class="buttons">
          <button type="button" class="btn" onclick={() => confirm(false)} data-default={s.danger ? "" : undefined}>Cancel</button>
          <button type="submit" class="btn" class:danger={s.danger} class:primary={!s.danger} data-default={s.danger ? undefined : ""}>{s.confirm}</button>
        </div>
      </div>
    </form>
  </Sheet>
{:else if ui.sheet?.kind === "conflict"}
  {@const s = ui.sheet}
  {#key s}
  <Sheet label="Name conflict" oncancel={() => conflict("stop")} width={480}>
    <form class="body" onsubmit={(e) => { e.preventDefault(); conflict("keepBoth"); }}>
      <div class="glyph"><FileIcon type={iconFor(s.existing)} size={36} /></div>
      <div class="text">
        <h2>An item named “{s.name}” already exists in “{s.folder}”.</h2>
        <p>
          {#if s.allowMerge}
            Replace it with the one you're {s.op === "move" ? "moving" : "copying"}, or merge the two folders (clashing files are replaced, the rest kept)?
          {:else if s.allowReplace}
            Do you want to replace it with the one you're {s.op === "move" ? "moving" : "copying"}?
          {:else}
            It can't be replaced: one is a folder and the other is a file.
          {/if}
        </p>
        <dl class="compare">
          <dt>Existing</dt><dd>{describe(s.existing)}</dd>
          {#if s.incoming}<dt>New</dt><dd>{describe(s.incoming)}</dd>{/if}
        </dl>
        {#if s.remaining > 1}
          <label class="check"><input type="checkbox" bind:checked={applyAll} /> Apply to all ({s.remaining} conflicts)</label>
        {/if}
        <div class="buttons">
          <button type="button" class="btn" onclick={() => conflict("stop")}>Stop</button>
          <span class="grow"></span>
          <button type="button" class="btn" onclick={() => conflict("skip")}>Skip</button>
          <button type="submit" class="btn primary" data-default>Keep Both</button>
          {#if s.allowMerge}<button type="button" class="btn" onclick={() => conflict("merge")}>Merge</button>{/if}
          {#if s.allowReplace}<button type="button" class="btn" onclick={() => conflict("replace")}>Replace</button>{/if}
        </div>
      </div>
    </form>
  </Sheet>
  {/key}
{:else if ui.sheet?.kind === "prefs"}
  <Sheet label="Preferences" oncancel={() => (ui.sheet = null)} width={440}>
    <form class="prefs" onsubmit={(e) => { e.preventDefault(); ui.sheet = null; }}>
      <h2>Preferences</h2>
      <fieldset>
        <legend>Appearance</legend>
        <div class="seg" role="radiogroup" aria-label="Appearance">
          {#each [["system", "System"], ["light", "Light"], ["dark", "Dark"]] as const as [value, label]}
            <label class:on={prefs.theme === value}>
              <input type="radio" name="theme" {value} bind:group={prefs.theme} />{label}
            </label>
          {/each}
        </div>
      </fieldset>
      <fieldset>
        <legend>New tabs open in</legend>
        <select class="field" bind:value={prefs.view} aria-label="Default view">
          <option value="icons">Icon view</option>
          <option value="list">List view</option>
          <option value="columns">Column view</option>
        </select>
      </fieldset>
      <fieldset>
        <legend>Files</legend>
        <label class="check"><input type="checkbox" checked={prefs.showHidden} onchange={(e) => setHidden(e.currentTarget.checked)} /> Show hidden files</label>
        <label class="check"><input type="checkbox" bind:checked={prefs.confirmDelete} /> Ask before deleting permanently</label>
      </fieldset>
      <fieldset>
        <legend>Sidebar shows</legend>
        <label class="check"><input type="checkbox" checked={prefs.sections.favourites} onchange={(e) => section("favourites", e.currentTarget.checked)} /> Favourites</label>
        <label class="check"><input type="checkbox" checked={prefs.sections.locations} onchange={(e) => section("locations", e.currentTarget.checked)} /> Locations</label>
        <label class="check"><input type="checkbox" checked={prefs.sections.servers} onchange={(e) => section("servers", e.currentTarget.checked)} /> Servers</label>
      </fieldset>
      <div class="buttons"><button type="submit" class="btn primary" data-default>Done</button></div>
    </form>
  </Sheet>
{:else if ui.sheet?.kind === "goto"}
  {@const s = ui.sheet}
  <Sheet label="Go to folder" oncancel={() => (ui.sheet = null)} width={460}>
    <form class="body" onsubmit={submitGoto}>
      <div class="glyph"><FolderOpen size={30} strokeWidth={1.6} /></div>
      <div class="text">
        <h2>Go to the folder on {sourceLabel(s.location.source)}:</h2>
        <input class="field mono" bind:value={gotoValue} spellcheck="false" autocomplete="off" aria-label="Path" placeholder="/path or ~/path" />
        <div class="buttons">
          <button type="button" class="btn" onclick={() => (ui.sheet = null)}>Cancel</button>
          <button type="submit" class="btn primary" disabled={busy || !gotoValue.trim()}>Go</button>
        </div>
      </div>
    </form>
  </Sheet>
{:else if ui.sheet?.kind === "info"}
  {@const { entry, location } = ui.sheet}
  <Sheet label="Info" oncancel={() => (ui.sheet = null)} width={400}>
    <div class="info">
      <div class="info-head">
        <FileIcon type={iconFor(entry)} size={56} link={entry.isSymlink} />
        <div class="info-title">
          <h2 class="selectable">{entry.name}</h2>
          <p>{kindLabel(entry)}{entry.kind === "file" ? ` — ${formatSize(entry.size)}` : ""}</p>
        </div>
      </div>
      <dl>
        <dt>Where</dt><dd class="selectable">{sourceLabel(location.source)}: {parentPath(entry.path) ?? "/"}</dd>
        {#if entry.kind === "file"}<dt>Size</dt><dd>{formatSize(entry.size)} ({entry.size.toLocaleString("en-GB")} bytes)</dd>{/if}
        <dt>Modified</dt><dd>{formatDateLong(entry.modified)}</dd>
        <dt>Permissions</dt><dd class="mono">{formatMode(entry.mode, entry.kind === "dir")}</dd>
        {#if entry.isSymlink}<dt>Alias</dt><dd>Symbolic link{entry.kind === "other" ? " (broken)" : ""}</dd>{/if}
        {#if entry.isHidden}<dt>Hidden</dt><dd>Yes</dd>{/if}
      </dl>
      <div class="buttons"><button class="btn primary" data-default onclick={() => (ui.sheet = null)}>Done</button></div>
    </div>
  </Sheet>
{:else if ui.sheet?.kind === "host"}
  {@const h = ui.sheet.host}
  <Sheet label="Server info" oncancel={() => (ui.sheet = null)} width={420}>
    <div class="info">
      <div class="info-head">
        <div class="glyph"><Server size={30} strokeWidth={1.6} /></div>
        <div class="info-title">
          <h2>{h.alias}</h2>
          <p><span class="dot {h.status.state}"></span> {h.status.state}</p>
        </div>
      </div>
      <dl>
        <dt>Host</dt><dd class="selectable mono">{h.hostname}:{h.port}</dd>
        <dt>User</dt><dd class="selectable">{h.user ?? "(default)"}</dd>
        <dt>Identity</dt><dd class="selectable mono">{h.identityFiles.length ? h.identityFiles.join("\n") : "ssh-agent / defaults"}</dd>
        {#if h.proxyJump}<dt>ProxyJump</dt><dd class="selectable mono">{h.proxyJump}</dd>{/if}
        <dt>Agent fwd</dt><dd>{h.forwardAgent ? "Yes" : "No"}</dd>
        {#if h.status.state === "failed"}<dt>Error</dt><dd class="selectable err">{h.status.error.message}</dd>{/if}
      </dl>
      <div class="buttons"><button class="btn primary" data-default onclick={() => (ui.sheet = null)}>Done</button></div>
    </div>
  </Sheet>
{/if}

<style>
  .body { display: flex; gap: 16px; }
  .glyph {
    flex: none;
    width: 48px;
    height: 48px;
    display: grid;
    place-items: center;
    border-radius: 12px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .glyph.warn { background: rgba(255, 159, 10, 0.16); color: #f29a0c; }
  .text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 8px; }
  h2 { margin: 0; font-size: 13px; font-weight: 650; line-height: 1.35; overflow-wrap: anywhere; }
  p { margin: 0; color: var(--fg-secondary); font-size: 12px; }
  .note { font-size: 11px; }
  code, .mono { font-family: var(--font-mono); font-size: 12px; }
  code {
    padding: 6px 8px;
    border-radius: var(--r-control);
    background: var(--bg-control);
    overflow-wrap: anywhere;
  }
  .path { font-family: var(--font-mono); overflow-wrap: anywhere; }
  .field { width: 100%; }
  .buttons { display: flex; justify-content: flex-end; gap: 8px; margin-top: 8px; }
  .info { display: flex; flex-direction: column; gap: 14px; }
  .info-head { display: flex; align-items: center; gap: 12px; }
  .info-title { min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .info-title p { display: flex; align-items: center; gap: 6px; }
  dl { display: grid; grid-template-columns: 90px 1fr; gap: 6px 12px; margin: 0; font-size: 12px; }
  dt { color: var(--fg-secondary); text-align: right; }
  dd { margin: 0; overflow-wrap: anywhere; white-space: pre-line; }
  .err { color: var(--danger); }
  .compare { grid-template-columns: 64px 1fr; }
  .prefs { display: flex; flex-direction: column; gap: 14px; }
  fieldset { display: flex; flex-direction: column; gap: 6px; margin: 0; padding: 0; border: 0; }
  legend { padding: 0; margin-bottom: 6px; font-size: var(--fs-small); font-weight: 600; color: var(--fg-secondary); }
  .seg { display: flex; padding: 2px; border-radius: 7px; background: var(--bg-control); }
  .seg label { flex: 1; text-align: center; padding: 3px 0; border-radius: 5px; }
  .seg label.on { background: var(--bg-window); box-shadow: var(--shadow-control); font-weight: 550; }
  .seg label:has(input:focus-visible) { outline: 2px solid var(--accent); outline-offset: -2px; }
  .seg input { position: absolute; opacity: 0; width: 0; height: 0; }
  select.field { align-self: flex-start; min-width: 180px; }
  .check { display: flex; align-items: center; gap: 6px; font-size: 12px; }
  .grow { flex: 1; }
</style>
