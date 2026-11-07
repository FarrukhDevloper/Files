<!-- Address bar (GNOME Files / Explorer style): breadcrumb buttons that turn
     into a path field on a click in empty space or Mod+L. Enter navigates,
     Escape restores, Tab completes folder names from the parent listing.
     A remote path shows its server as a chip; "server:/path" switches. -->
<script lang="ts">
  import { tick } from "svelte";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import Ellipsis from "@lucide/svelte/icons/ellipsis";
  import House from "@lucide/svelte/icons/house";
  import Laptop from "@lucide/svelte/icons/laptop-minimal";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import Server from "@lucide/svelte/icons/server";
  import FileIcon from "./FileIcon.svelte";
  import { StartIcon } from "./places";
  import { goToPath } from "$lib/actions";
  import { api } from "$lib/ipc/api";
  import type { Entry, SourceId } from "$lib/ipc/types";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { hosts } from "$lib/stores/hosts.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { toasts } from "$lib/stores/toasts.svelte";
  import { ui, type MenuItem } from "$lib/stores/ui.svelte";
  import { ancestors, baseName, sourceKey } from "$lib/util/path";

  const MAX_SUGGESTIONS = 12;
  const SUGGEST_DELAY = 120;

  const b = $derived(tabs.current);

  // ---- breadcrumbs ----

  type CrumbIcon = "home" | "homeFolder" | "computer" | "server" | "drive" | "folder";
  interface Crumb { path: string | null; label: string; icon: CrumbIcon }

  const crumbs = $derived.by((): Crumb[] => {
    const loc = b.location;
    if (!loc) return b.home ? [{ path: null, label: "Home", icon: "home" }] : [];
    const chain = ancestors(loc.path);
    const folder = (path: string): Crumb => ({ path, label: baseName(path), icon: "folder" });
    const src = loc.source;
    if (src.kind === "ssh") {
      return chain.map((p, i) => (i === 0 ? { path: p, label: src.host, icon: "server" } : folder(p)));
    }
    // Inside the home folder, start there (like GNOME Files).
    const home = hosts.homeFolder;
    const at = home ? chain.indexOf(home) : -1;
    if (at >= 0) {
      return chain.slice(at).map((p, i) => (i === 0 ? { path: p, label: baseName(p) || p, icon: "homeFolder" } : folder(p)));
    }
    return chain.map((p, i): Crumb =>
      i === 0 ? { path: p, label: "Computer", icon: "computer" }
        : /^[A-Za-z]:\/$/.test(p) ? { path: p, label: p.slice(0, 2), icon: "drive" }
        : folder(p),
    );
  });

  const srcKey = $derived(b.location ? sourceKey(b.location.source) : "");

  function crumbClick(c: Crumb) {
    const loc = b.location;
    if (c.path === null || !loc) return;
    if (c.path !== loc.path) void b.go({ source: loc.source, path: c.path });
  }

  function crumbMenu(e: MouseEvent, c: Crumb) {
    const loc = b.location;
    if (c.path === null || !loc) return;
    const target = { source: loc.source, path: c.path };
    const prefix = loc.source.kind === "ssh" ? `${loc.source.host}:` : "";
    const items: MenuItem[] = [
      { label: "Open", run: () => crumbClick(c) },
      { label: "Open in New Tab", run: () => void tabs.open(b, target) },
      "separator",
      { label: "Copy Path", run: () => void navigator.clipboard.writeText(prefix + c.path).catch(() => toasts.info("Couldn't access the system clipboard")) },
    ];
    ui.openMenu(e, items);
  }

  // ---- folding: when the path doesn't fit, the root shows as an icon, the
  // last crumbs keep their full width (only the very last may ellipsize) and
  // the ones in between fold into a "…" menu. Widths come from a hidden
  // measuring row; the space from a ResizeObserver.

  let nav = $state<HTMLElement>();
  let measurer = $state<HTMLElement>();
  let avail = $state(0);
  let size = $state.raw({ crumbs: [] as number[], rootIcon: 0, sep: 0, more: 0 });

  function measure() {
    if (!measurer) return;
    const w = (sel: string) => measurer!.querySelector<HTMLElement>(sel)?.offsetWidth ?? 0;
    size = {
      crumbs: [...measurer.querySelectorAll<HTMLElement>("[data-crumb]")].map((el) => el.offsetWidth),
      rootIcon: w("[data-root-icon]"),
      sep: w("[data-sep]"),
      more: w("[data-more]"),
    };
  }

  // Effects run after the DOM update, so the measuring row is current.
  $effect(() => {
    void crumbs;
    measure();
  });

  $effect(() => {
    const el = nav;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => {
      avail = entry!.contentRect.width;
      measure(); // fonts or zoom may have changed widths too
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  /** compact: root as icon; crumbs 1..from-1 fold into "…". */
  const layout = $derived.by(() => {
    const n = crumbs.length;
    const w = size.crumbs;
    if (n < 2 || w.length !== n || avail <= 0) return { compact: false, from: 1 };
    const sum = (from: number) => w.slice(from).reduce((a, x) => a + x, 0) + size.sep * (n - 1 - from);
    if (w[0]! + size.sep + sum(1) <= avail) return { compact: false, from: 1 };
    // Show as many trailing crumbs as fit; the last one always.
    for (let from = 1; from < n - 1; from++) {
      const more = from > 1 ? size.more + size.sep : 0;
      if (size.rootIcon + size.sep + more + sum(from) <= avail) return { compact: true, from };
    }
    return { compact: true, from: n - 1 };
  });
  const folded = $derived(layout.compact ? crumbs.slice(1, layout.from) : []);

  function foldedMenu(e: MouseEvent) {
    ui.openMenu(e, folded.map((c): MenuItem => ({ label: c.label, run: () => crumbClick(c) })));
  }

  // ---- editing ----

  let editing = $state(false);
  let text = $state("");
  /** Server the typed path is on (the chip); null = this computer. */
  let host = $state<string | null>(null);
  let busy = $state(false);
  let input = $state<HTMLInputElement>();

  async function startEdit() {
    const loc = b.location;
    host = loc?.source.kind === "ssh" ? loc.source.host : null;
    text = loc ? loc.path : "";
    suggestions = [];
    highlight = -1;
    editing = true;
    await tick();
    input?.focus();
    input?.select();
  }
  ui.editAddress = () => void startEdit();

  function stopEdit(refocus: boolean) {
    editing = false;
    suggestions = [];
    highlight = -1;
    suggestSeq++;
    dirCache.clear();
    if (refocus) ui.focusFiles();
  }

  interface Target { source: SourceId; path: string; prefix: string }

  /** "web1:/var/www" names a server; anything else is on the chip's source.
   *  Needs 2+ characters before ":" so "C:/Users" stays a local path. */
  function parse(raw: string): Target {
    const m = /^([\w.-]{2,}):(.*)$/.exec(raw);
    if (m && hosts.get(m[1]!)) return { source: { kind: "ssh", host: m[1]! }, path: m[2]!, prefix: `${m[1]}:` };
    return { source: host ? { kind: "ssh", host } : { kind: "local" }, path: raw, prefix: "" };
  }

  async function submit(raw: string) {
    if (busy) return;
    const t = parse(raw.trim());
    if (!t.path.trim()) t.path = "~";
    busy = true;
    try {
      if (t.source.kind === "ssh") {
        try {
          await hosts.connect(t.source.host);
        } catch (e) {
          toasts.error(e, `Couldn't connect to “${t.source.host}”`);
          return;
        }
      }
      if (await goToPath(b, t.source, t.path)) stopEdit(true);
    } finally {
      busy = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    const el = e.currentTarget as HTMLInputElement;
    if (e.key === "Escape") {
      e.preventDefault();
      stopEdit(true);
    } else if (e.key === "Enter") {
      e.preventDefault();
      void submit(highlight >= 0 ? suggestions[highlight]!.text : text);
    } else if (e.key === "Tab" && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault();
      void complete(e.shiftKey ? -1 : 1);
    } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && suggestions.length) {
      e.preventDefault();
      const n = suggestions.length;
      highlight = e.key === "ArrowDown" ? (highlight + 1) % n : (highlight <= 0 ? n : highlight) - 1;
    } else if (e.key === "ArrowDown") {
      // ↓ with no list: show the folders here.
      e.preventDefault();
      void suggest(text.endsWith("/") ? text : `${text}/`);
    } else if (e.key === "Backspace" && host && el.selectionStart === 0 && el.selectionEnd === 0) {
      // Backspace into the chip: the path is on this computer instead.
      e.preventDefault();
      host = null;
      scheduleSuggest();
    }
  }

  // ---- completion ----

  interface Suggestion { name: string; text: string }
  let suggestions = $state.raw<Suggestion[]>([]);
  let highlight = $state(-1);
  let suggestSeq = 0;
  let suggestTimer: ReturnType<typeof setTimeout> | undefined;
  const dirCache = new Map<string, Promise<Entry[]>>();
  const homeCache = new Map<string, Promise<string>>();
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

  function listDirs(source: SourceId, dir: string): Promise<Entry[]> {
    const key = `${sourceKey(source)}|${dir}`;
    let p = dirCache.get(key);
    if (!p) {
      p = api.listDir({ source, path: dir }).then(
        (list) => list.filter((e) => e.kind === "dir").sort((x, y) => collator.compare(x.name, y.name)),
        () => [],
      );
      dirCache.set(key, p);
    }
    return p;
  }

  function homeOf(source: SourceId): Promise<string> {
    const key = sourceKey(source);
    let p = homeCache.get(key);
    if (!p) {
      p = api.homeDir(source);
      p.catch(() => homeCache.delete(key));
      homeCache.set(key, p);
    }
    return p;
  }

  /** Folders matching the last segment of `raw`, each as the full text to put in the field. */
  async function matches(raw: string): Promise<Suggestion[] | null> {
    const t = parse(raw);
    const cut = t.path.lastIndexOf("/");
    if (cut < 0) return null;
    // `typedDir` keeps what the user wrote ("~/", "web1:/srv/"); `dir` is absolute.
    const typedDir = t.path.slice(0, cut + 1);
    const partial = t.path.slice(cut + 1).toLocaleLowerCase();
    let dir = cut === 0 ? "/" : t.path.slice(0, cut);
    if (/^[A-Za-z]:$/.test(dir)) dir += "/";
    if (dir === "~" || dir.startsWith("~/")) {
      try {
        const home = await homeOf(t.source);
        dir = dir === "~" ? home : `${home.replace(/\/$/, "")}/${dir.slice(2)}`;
      } catch {
        return null;
      }
    }
    if (t.source.kind === "ssh" && !hosts.isConnected(t.source.host)) return null;
    const showHidden = b.showHidden || partial.startsWith(".");
    return (await listDirs(t.source, dir))
      .filter((e) => (showHidden || !e.isHidden) && e.name.toLocaleLowerCase().startsWith(partial))
      .slice(0, MAX_SUGGESTIONS)
      .map((e) => ({ name: e.name, text: t.prefix + typedDir + e.name }));
  }

  async function suggest(raw: string) {
    const seq = ++suggestSeq;
    const found = await matches(raw);
    if (seq !== suggestSeq || !editing) return;
    // Nothing more to offer when the only match is what's typed already.
    suggestions = found && !(found.length === 1 && found[0]!.text === raw) ? found : [];
    highlight = -1;
  }

  function scheduleSuggest() {
    clearTimeout(suggestTimer);
    suggestTimer = setTimeout(() => void suggest(text), SUGGEST_DELAY);
  }

  /** Tab: one match → take it (and list its subfolders); several → extend to
   *  their common prefix, then cycle through them. Presses queue up, and
   *  text typed while a listing loads is kept after the completion. */
  let completing = Promise.resolve();
  function complete(dir: 1 | -1) {
    clearTimeout(suggestTimer);
    completing = completing.then(() => completeNow(dir));
  }

  async function completeNow(dir: 1 | -1) {
    if (!editing) return;
    if (highlight >= 0) {
      cycle(dir);
      return;
    }
    const typed = text;
    const seq = ++suggestSeq;
    const found = await matches(typed);
    if (!editing || !found?.length || !text.startsWith(typed)) return;
    const extra = text.slice(typed.length);
    if (found.length === 1) {
      text = `${found[0]!.text}/${extra}`;
      await suggest(text);
      return;
    }
    if (seq === suggestSeq) {
      suggestions = found;
      highlight = -1;
    }
    const prefix = commonPrefix(found);
    if (prefix.length > typed.length) text = prefix + extra;
    else if (!extra) cycle(dir);
  }

  function cycle(dir: 1 | -1) {
    const n = suggestions.length;
    if (!n) return;
    highlight = dir > 0 ? (highlight + 1) % n : (highlight <= 0 ? n : highlight) - 1;
    text = suggestions[highlight]!.text;
  }

  function commonPrefix(list: Suggestion[]): string {
    let p = list[0]!.text;
    for (const s of list) {
      let i = 0;
      while (i < p.length && i < s.text.length && p[i]!.toLocaleLowerCase() === s.text[i]!.toLocaleLowerCase()) i++;
      p = p.slice(0, i);
    }
    return p;
  }

  function onBarDown(e: MouseEvent) {
    // Empty space (not a crumb) switches to the text field.
    if (e.button !== 0 || editing || (e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    void startEdit();
  }

  function crumbIcon(c: Crumb) {
    switch (c.icon) {
      case "home": return StartIcon;
      case "homeFolder": return House;
      case "computer": return Laptop;
      case "server": return Server;
      case "drive": return HardDrive;
      default: return null;
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="address" class:editing data-tauri-drag-region="false" onmousedown={onBarDown}>
  {#if editing}
    {#if host}
      <span class="chip" title="On {host} — Backspace at the start for this computer"><Server size={13} strokeWidth={2} />{host}:</span>
    {/if}
    <input
      bind:this={input}
      bind:value={text}
      class="path"
      spellcheck="false"
      autocomplete="off"
      aria-label="Path"
      placeholder={host ? "/path or ~/path" : "/path, ~/path or server:/path"}
      role="combobox"
      aria-expanded={suggestions.length > 0}
      aria-controls="address-suggestions"
      aria-autocomplete="list"
      aria-activedescendant={highlight >= 0 ? `address-s${highlight}` : undefined}
      readonly={busy}
      {onkeydown}
      oninput={() => {
        // The list is stale until the next suggest(); Tab must not cycle it.
        highlight = -1;
        scheduleSuggest();
      }}
      onblur={() => stopEdit(false)}
    />
    {#if busy}<span class="spinner" aria-label="Opening"></span>{/if}
    {#if suggestions.length}
      <ul class="suggestions" id="address-suggestions" role="listbox" aria-label="Folders">
        {#each suggestions as s, i (s.text)}
          <!-- Mouse only: the field keeps focus and the keyboard. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <li
            id="address-s{i}"
            role="option"
            aria-selected={i === highlight}
            class:on={i === highlight}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => void submit(s.text)}
          >
            <FileIcon type="folder" size={18} />
            <span>{s.name}</span>
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <nav class="crumbs" aria-label="Path" bind:this={nav}>
      {#each crumbs as c, i (c.path ?? "home")}
        {#if i === 0 || i >= layout.from}
          {@const Icon = crumbIcon(c)}
          {@const last = i === crumbs.length - 1}
          {@const iconOnly = i === 0 && layout.compact}
          {#if i > 0}<span class="sep" aria-hidden="true"><ChevronRight size={14} strokeWidth={2} /></span>{/if}
          {#if i > 0 && i === layout.from && folded.length}
            <button class="crumb more" title="Folders above" aria-label="{folded.length} folders above" aria-haspopup="menu" onclick={foldedMenu}>
              <Ellipsis size={16} strokeWidth={2} />
            </button>
            <span class="sep" aria-hidden="true"><ChevronRight size={14} strokeWidth={2} /></span>
          {/if}
          <button
            class="crumb"
            class:last
            class:icon-only={iconOnly}
            class:drop={c.path !== null && dnd.target?.key === `${srcKey}|${c.path}`}
            data-drop={c.path !== null ? "dir" : undefined}
            data-drop-source={c.path !== null ? srcKey : undefined}
            data-drop-path={c.path ?? undefined}
            aria-current={last ? "location" : undefined}
            aria-label={iconOnly ? c.label : undefined}
            title={c.path ?? "Home"}
            onclick={() => crumbClick(c)}
            oncontextmenu={(e) => crumbMenu(e, c)}
          >
            {#if Icon}<Icon size={15} strokeWidth={1.9} />{:else}<FileIcon type="folder" size={16} />{/if}
            {#if !iconOnly}<span>{c.label}</span>{/if}
          </button>
        {/if}
      {/each}
    </nav>
    <!-- Natural widths for the folding layout; never visible. -->
    <div class="measure" aria-hidden="true">
      <div class="measure-row" bind:this={measurer}>
        {#each crumbs as c, i (c.path ?? "home")}
          {@const Icon = crumbIcon(c)}
          <span class="crumb" class:last={i === crumbs.length - 1} data-crumb>
            {#if Icon}<Icon size={15} strokeWidth={1.9} />{:else}<FileIcon type="folder" size={16} />{/if}
            <span>{c.label}</span>
          </span>
        {/each}
        {#if crumbs[0]}
          {@const RootIcon = crumbIcon(crumbs[0])}
          <span class="crumb icon-only" data-root-icon>{#if RootIcon}<RootIcon size={15} strokeWidth={1.9} />{/if}</span>
        {/if}
        <span class="crumb more" data-more><Ellipsis size={16} strokeWidth={2} /></span>
        <span class="sep" data-sep><ChevronRight size={14} strokeWidth={2} /></span>
      </div>
    </div>
  {/if}
</div>

<style>
  .address {
    position: relative;
    flex: 1 1 auto;
    min-width: 120px;
    display: flex;
    align-items: center;
    height: 32px;
    padding: 0 4px;
    border-radius: 8px;
    background: var(--bg-control);
    cursor: text;
  }
  .address.editing { background: var(--bg-field); box-shadow: 0 0 0 0.5px var(--accent), var(--focus-ring); padding-left: 6px; }
  .crumbs {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    overflow: hidden;
    white-space: nowrap;
  }
  .crumb {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    height: 26px;
    padding: 0 7px;
    border-radius: 6px;
    color: var(--fg-secondary);
    white-space: nowrap;
    cursor: default;
  }
  .crumb span { overflow: hidden; text-overflow: ellipsis; }
  .crumb:hover { background: var(--bg-hover); color: var(--fg-primary); }
  .crumb:active { background: var(--bg-pressed); }
  /* Only the current folder may shrink (ellipsized) when even it won't fit. */
  .crumb.last { flex: 0 1 auto; min-width: 0; color: var(--fg-primary); font-weight: 600; }
  .crumb.icon-only, .crumb.more { padding: 0 6px; }
  .measure { position: absolute; left: 0; top: 0; width: 0; height: 0; overflow: hidden; visibility: hidden; pointer-events: none; }
  .measure-row { display: flex; width: max-content; }
  .crumb.drop { background: var(--accent); color: var(--accent-fg); }
  .crumb :global(svg) { flex: none; }
  .sep { display: grid; place-items: center; flex: none; color: var(--fg-tertiary); }

  .chip {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 22px;
    margin-right: 2px;
    padding: 0 7px;
    border-radius: 5px;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
    font-size: var(--fs-secondary);
  }
  .path {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    padding: 0 4px;
    background: none;
    font-family: var(--font-mono);
    font-size: var(--fs-secondary);
  }
  .path:focus-visible { outline: none; }
  .address .spinner { flex: none; margin: 0 6px; }

  .suggestions {
    position: absolute;
    z-index: 40;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    max-height: 340px;
    margin: 0;
    padding: 5px;
    overflow-y: auto;
    list-style: none;
    border-radius: var(--r-popover);
    background: var(--bg-popover);
    box-shadow: var(--shadow-popover);
    cursor: default;
  }
  li {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 8px;
    border-radius: 5px;
  }
  li span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  li:hover { background: var(--bg-hover); }
  li.on { background: var(--accent); color: var(--accent-fg); }
</style>
