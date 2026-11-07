<!-- Home start page: this computer's places and drives, and every server as
     a card (status, address, disk usage when connected) plus "Add Server". -->
<script lang="ts">
  import { untrack } from "svelte";
  import Server from "@lucide/svelte/icons/server";
  import Plus from "@lucide/svelte/icons/plus";
  import Ellipsis from "@lucide/svelte/icons/ellipsis";
  import { placeIcons, placeLabel } from "./places";
  import { addServer, disconnectHost, hostAddress, hostMenu, openHost, openPlace } from "$lib/actions";
  import { api } from "$lib/ipc/api";
  import type { DiskSpace, HostInfo, SourceId } from "$lib/ipc/types";
  import { dnd } from "$lib/stores/dnd.svelte";
  import { hosts } from "$lib/stores/hosts.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { errorTitle } from "$lib/util/errors";
  import { formatSize } from "$lib/util/format";

  const b = $derived(tabs.current);
  const local: SourceId = { kind: "local" };

  // ---- disk usage: local drives once, servers while connected ----

  let usage = $state.raw(new Map<string, DiskSpace>());
  const asked = new Set<string>();

  function load(key: string, get: () => Promise<DiskSpace | null>) {
    if (asked.has(key)) return;
    asked.add(key);
    get().then(
      (d) => { if (d && asked.has(key)) usage = new Map(usage).set(key, d); },
      () => {},
    );
  }

  $effect(() => {
    const drives = hosts.locations;
    const servers = hosts.list.map((h) => [h.alias, h.status.state] as const);
    untrack(() => {
      for (const p of drives) load(`local|${p.path}`, () => api.diskFree({ source: local, path: p.path }));
      for (const [alias, state] of servers) {
        const key = `ssh:${alias}`;
        if (state === "connected") {
          load(key, async () => {
            const source: SourceId = { kind: "ssh", host: alias };
            return api.diskFree({ source, path: await api.homeDir(source) });
          });
        } else if (asked.delete(key) && usage.has(key)) {
          const next = new Map(usage);
          next.delete(key);
          usage = next;
        }
      }
    });
  });

  function statusText(h: HostInfo): string {
    switch (h.status.state) {
      case "connected": return "Connected";
      case "connecting": return "Connecting…";
      case "failed": return errorTitle(h.status.error.code);
      default: return "Not connected";
    }
  }
</script>

{#snippet usageBar(d: DiskSpace | undefined)}
  {#if d && d.total > 0}
    {@const used = 1 - d.free / d.total}
    <span class="usage">
      <span class="bar" class:full={used > 0.9}><span style:width="{Math.round(used * 100)}%"></span></span>
      <span class="usage-text">{formatSize(d.free)} free of {formatSize(d.total)}</span>
    </span>
  {/if}
{/snippet}

<div class="home">
  <section>
    <h2>This Computer</h2>
    {#if hosts.favourites.length}
      <div class="places">
        {#each hosts.favourites as p (p.path)}
          {@const Icon = placeIcons[p.kind]}
          <button
            class="place"
            class:drop={dnd.target?.key === `local|${p.path}`}
            data-drop="dir"
            data-drop-source="local"
            data-drop-path={p.path}
            title={p.path}
            onclick={() => openPlace(b, p)}
          >
            <span class="place-icon"><Icon size={22} strokeWidth={1.7} /></span>
            <span class="label">{placeLabel(p)}</span>
          </button>
        {/each}
      </div>
    {/if}
    {#if hosts.locations.length}
      <div class="cards">
        {#each hosts.locations as p (p.path)}
          {@const Icon = placeIcons[p.kind]}
          <button
            class="card drive"
            class:drop={dnd.target?.key === `local|${p.path}`}
            data-drop="dir"
            data-drop-source="local"
            data-drop-path={p.path}
            title={p.path}
            onclick={() => openPlace(b, p)}
          >
            <span class="big-icon"><Icon size={26} strokeWidth={1.6} /></span>
            <span class="text">
              <strong>{p.label}</strong>
              {@render usageBar(usage.get(`local|${p.path}`))}
            </span>
          </button>
        {/each}
      </div>
    {/if}
  </section>

  <section>
    <h2>Servers</h2>
    <div class="cards">
      {#each hosts.list as h (h.alias)}
        {@const state = h.status.state}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="card server"
          class:drop={dnd.target?.key === `host:${h.alias}`}
          data-drop="host"
          data-drop-host={h.alias}
          oncontextmenu={(e) => hostMenu(b, e, h.alias)}
        >
          <button class="server-main" title={h.status.state === "failed" ? h.status.error.message : `Open ${h.alias}`} onclick={() => void openHost(b, h.alias)}>
            <span class="big-icon">
              <Server size={26} strokeWidth={1.6} />
              <span class="dot {state}"></span>
            </span>
            <span class="text">
              <strong>{h.alias}</strong>
              <span class="addr" title={hostAddress(h)}>{hostAddress(h)}{h.proxyJump ? ` via ${h.proxyJump}` : ""}</span>
              <span class="status" class:failed={state === "failed"}>{statusText(h)}</span>
            </span>
          </button>
          {@render usageBar(usage.get(`ssh:${h.alias}`))}
          <div class="actions">
            {#if state === "connected"}
              <button class="btn primary" onclick={() => void openHost(b, h.alias)}>Open</button>
              <button class="btn" onclick={() => void disconnectHost(b, h.alias)}>Disconnect</button>
            {:else if state === "connecting"}
              <button class="btn" disabled>Connecting…</button>
            {:else}
              <button class="btn primary" onclick={() => void openHost(b, h.alias)}>Connect</button>
            {/if}
            <span class="grow"></span>
            <button class="tool" aria-label="More for {h.alias}" aria-haspopup="menu" onclick={(e) => hostMenu(b, e, h.alias)}>
              <Ellipsis size={18} />
            </button>
          </div>
        </div>
      {/each}
      <button class="card add" onclick={addServer}>
        <span class="add-icon"><Plus size={22} strokeWidth={2} /></span>
        <strong>Add Server</strong>
        <span class="hint">Key, password or ssh-agent</span>
      </button>
    </div>
  </section>
</div>

<style>
  .home {
    height: 100%;
    overflow-y: auto;
    padding: 24px 32px 40px;
  }
  section { max-width: 1120px; margin: 0 auto 28px; }
  h2 {
    margin: 0 0 12px;
    font-size: var(--fs-title);
    font-weight: 650;
  }

  .places {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 8px;
    margin-bottom: 12px;
  }
  .place {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 48px;
    padding: 0 12px;
    border-radius: 10px;
    background: var(--bg-row-alt);
    text-align: left;
  }
  .place:hover { background: var(--bg-hover); }
  .place:active { background: var(--bg-pressed); }
  .place-icon { display: grid; place-items: center; color: var(--accent); }
  .label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }

  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 12px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    padding: 14px;
    border-radius: 12px;
    background: var(--bg-row-alt);
    box-shadow: 0 0 0 0.5px var(--border);
    text-align: left;
  }
  .card.drop, .place.drop { background: var(--accent-soft); box-shadow: inset 0 0 0 2px var(--accent); }
  .drive { flex-direction: row; align-items: center; gap: 12px; }
  .drive:hover { background: var(--bg-hover); }

  .big-icon {
    position: relative;
    flex: none;
    display: grid;
    place-items: center;
    width: 46px;
    height: 46px;
    border-radius: 11px;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .big-icon .dot {
    position: absolute;
    right: -2px;
    bottom: -2px;
    width: 12px;
    height: 12px;
    box-shadow: 0 0 0 2.5px var(--bg-row-alt);
  }
  .text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
  .text strong, .add strong { font-size: var(--fs-ui); font-weight: 650; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .addr {
    color: var(--fg-secondary);
    font-family: var(--font-mono);
    font-size: var(--fs-small);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint { color: var(--fg-secondary); font-size: var(--fs-small); }
  .status { color: var(--fg-secondary); font-size: var(--fs-small); }
  .status.failed { color: var(--danger); }

  .server-main { display: flex; align-items: center; gap: 12px; text-align: left; border-radius: 8px; }
  .actions { display: flex; align-items: center; gap: 8px; }
  .actions .btn { min-width: 0; }
  .grow { flex: 1; }

  .usage { display: flex; flex-direction: column; gap: 4px; }
  .bar { height: 5px; border-radius: 3px; background: var(--progress-track); overflow: hidden; }
  .bar span { display: block; height: 100%; border-radius: 3px; background: var(--accent); }
  .bar.full span { background: var(--danger); }
  .usage-text { color: var(--fg-secondary); font-size: var(--fs-small); }

  .add {
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 140px;
    background: none;
    box-shadow: none;
    border: 1.5px dashed var(--border-strong);
    color: var(--fg-secondary);
  }
  .add:hover { background: var(--bg-hover); color: var(--fg-primary); }
  .add-icon {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border-radius: 50%;
    background: var(--bg-control);
    color: var(--accent);
  }
</style>
