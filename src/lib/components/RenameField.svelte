<script lang="ts">
  import { onMount } from "svelte";
  import type { Entry } from "$lib/ipc/types";

  let { entry, oncommit, oncancel, center = false }: {
    entry: Entry;
    oncommit: (name: string) => void;
    oncancel: () => void;
    center?: boolean;
  } = $props();

  // svelte-ignore state_referenced_locally
  let value = $state(entry.name);
  let input = $state<HTMLInputElement>();
  let done = false;

  onMount(() => {
    if (!input) return;
    input.focus();
    // Select the stem only, like Finder: "report" of "report.pdf".
    const dot = entry.kind === "dir" ? -1 : entry.name.lastIndexOf(".");
    input.setSelectionRange(0, dot > 0 ? dot : entry.name.length);
  });

  function commit() {
    if (done) return;
    done = true;
    oncommit(value);
  }

  function cancel() {
    if (done) return;
    done = true;
    oncancel();
  }

  function onkeydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      commit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancel();
    }
  }
</script>

<input
  bind:this={input}
  bind:value
  class="rename"
  class:center
  spellcheck="false"
  autocomplete="off"
  aria-label="New name"
  {onkeydown}
  onblur={commit}
  onmousedown={(e) => e.stopPropagation()}
  ondblclick={(e) => e.stopPropagation()}
  oncontextmenu={(e) => e.stopPropagation()}
/>

<style>
  .rename {
    min-width: 80px;
    width: 100%;
    height: 20px;
    margin: 0 -3px;
    padding: 0 3px;
    border: 0;
    border-radius: 3px;
    background: var(--bg-field);
    color: var(--fg-primary);
    box-shadow: 0 0 0 1px var(--accent), var(--focus-ring);
  }
  .center { text-align: center; margin: 0; }
</style>
