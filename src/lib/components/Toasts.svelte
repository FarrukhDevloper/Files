<script lang="ts">
  import CircleExclamation from "@lucide/svelte/icons/circle-exclamation-point";
  import Info from "@lucide/svelte/icons/info";
  import X from "@lucide/svelte/icons/x";
  import { toasts } from "$lib/stores/toasts.svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each toasts.list as t (t.id)}
    <div
      class="toast {t.tone}"
      role={t.tone === "error" ? "alert" : "status"}
      onpointerenter={() => toasts.hold(t.id)}
      onpointerleave={() => toasts.release(t.id)}
    >
      <span class="icon">
        {#if t.tone === "error"}<CircleExclamation size={16} />{:else}<Info size={16} />{/if}
      </span>
      <div class="text">
        <strong>{t.title}</strong>
        {#if t.message}<p class="selectable">{t.message}</p>{/if}
        {#if t.action}
          {@const action = t.action}
          <button class="action" onclick={() => { toasts.dismiss(t.id); action.run(); }}>{action.label}</button>
        {/if}
      </div>
      <button class="close" aria-label="Dismiss" onclick={() => toasts.dismiss(t.id)}><X size={13} /></button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 14px;
    bottom: calc(var(--pathbar-h) + 12px);
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 340px;
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    gap: 10px;
    padding: 10px 10px 10px 12px;
    border-radius: var(--r-popover);
    background: var(--bg-popover);
    box-shadow: var(--shadow-popover);
    animation: rise 0.16s ease-out;
  }
  .icon { flex: none; padding-top: 1px; color: var(--accent); }
  .error .icon { color: var(--danger); }
  .text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  strong { font-weight: 600; overflow-wrap: anywhere; }
  p { margin: 0; font-size: var(--fs-secondary); color: var(--fg-secondary); white-space: pre-line; overflow-wrap: anywhere; max-height: 7.5em; overflow: hidden; }
  .action { align-self: flex-start; margin-top: 4px; color: var(--accent); font-weight: 550; font-size: var(--fs-secondary); }
  .close { flex: none; align-self: flex-start; display: grid; place-items: center; width: 18px; height: 18px; border-radius: 50%; color: var(--fg-tertiary); }
  .close:hover { background: var(--bg-hover); color: var(--fg-primary); }
  @keyframes rise { from { transform: translateY(6px); opacity: 0; } }
</style>
