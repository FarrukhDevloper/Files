<script lang="ts">
  import X from "@lucide/svelte/icons/x";
  import Plus from "@lucide/svelte/icons/plus";
  import Server from "@lucide/svelte/icons/server";
  import { newTab } from "$lib/actions";
  import { dnd, dropKey } from "$lib/stores/dnd.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { sourceKey } from "$lib/util/path";
  import { shortcutLabel } from "$lib/util/platform";

  function onauxclick(e: MouseEvent, i: number) {
    if (e.button === 1) {
      e.preventDefault();
      tabs.close(i);
    }
  }
</script>

<div class="tabbar" role="tablist" aria-label="Tabs">
  {#each tabs.list as t, i (t)}
    {@const remote = t.location?.source.kind === "ssh" ? t.location.source.host : null}
    <div
      class="tab"
      class:active={i === tabs.active}
      class:drop={!!t.location && dnd.target?.key === dropKey(t.location)}
      data-drop={t.location ? "dir" : undefined}
      data-drop-source={t.location ? sourceKey(t.location.source) : undefined}
      data-drop-path={t.location?.path}
      role="tab"
      tabindex="-1"
      aria-selected={i === tabs.active}
      title={remote ? `${remote}: ${t.location?.path}` : t.location?.path}
      onmousedown={(e) => {
        // Keep keyboard focus in the file view, like the toolbar buttons.
        if (e.button !== 0) return;
        e.preventDefault();
        tabs.select(i);
      }}
      onauxclick={(e) => onauxclick(e, i)}
    >
      <button class="close" aria-label="Close tab" title="Close Tab ({shortcutLabel('Mod+W')})" onclick={() => tabs.close(i)}>
        <X size={11} strokeWidth={2.4} />
      </button>
      <span class="title">
        {#if remote}<Server size={11} strokeWidth={2} />{/if}
        <span>{t.title}</span>
      </span>
    </div>
  {/each}
  <button class="add" aria-label="New tab" title="New Tab ({shortcutLabel('Mod+T')})" onclick={() => newTab()}>
    <Plus size={14} strokeWidth={2} />
  </button>
</div>

<style>
  .tabbar {
    flex: none;
    display: flex;
    height: 32px;
    background: var(--bg-sidebar);
    border-bottom: 1px solid var(--border);
  }
  .tab {
    position: relative;
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 26px;
    color: var(--fg-secondary);
    font-size: var(--fs-secondary);
  }
  .tab + .tab { border-left: 1px solid var(--border); }
  .tab:hover:not(.active) { background: var(--bg-hover); }
  .tab.active { background: var(--bg-toolbar); color: var(--fg-primary); font-weight: 550; }
  .tab.drop { background: var(--accent-soft); box-shadow: inset 0 0 0 2px var(--accent); }
  .title { display: flex; align-items: center; gap: 4px; min-width: 0; }
  .title span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .close {
    position: absolute;
    left: 6px;
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    color: var(--fg-secondary);
    opacity: 0;
  }
  .tab:hover .close, .close:focus-visible { opacity: 1; }
  .close:hover { background: var(--bg-pressed); color: var(--fg-primary); }
  .add {
    flex: none;
    display: grid;
    place-items: center;
    width: 30px;
    border-left: 1px solid var(--border);
    color: var(--fg-secondary);
  }
  .add:hover { background: var(--bg-hover); color: var(--fg-primary); }
</style>
