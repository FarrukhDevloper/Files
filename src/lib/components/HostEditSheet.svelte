<!-- Add / edit an app-managed server (api.saveHost). Servers from
     ~/.ssh/config are edited in the user's editor instead. -->
<script lang="ts">
  import { untrack } from "svelte";
  import Server from "@lucide/svelte/icons/server";
  import Sheet from "./Sheet.svelte";
  import { openHost, openStart } from "$lib/actions";
  import { api, isTauri } from "$lib/ipc/api";
  import type { HostDraft, HostInfo } from "$lib/ipc/types";
  import { hosts } from "$lib/stores/hosts.svelte";
  import { listings } from "$lib/stores/listings";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { errorTitle, toVfsError } from "$lib/util/errors";
  import { pickFile } from "$lib/util/window";

  let { host }: { host: HostInfo | null } = $props();

  type Auth = "key" | "password" | "agent";
  const AUTH: [Auth, string][] = [["key", "Key file"], ["password", "Password"], ["agent", "ssh-agent"]];

  // Initial values only: the sheet is re-created for every host.
  const initial = untrack(() => host);
  let alias = $state(initial?.alias ?? "");
  let hostname = $state(initial?.hostname ?? "");
  let user = $state(initial?.user ?? "");
  let port = $state(String(initial?.port ?? 22));
  let auth = $state<Auth>(!initial ? "key" : initial.identityFiles.length ? "key" : initial.hasSavedSecret ? "password" : "agent");
  let keyPath = $state(initial?.identityFiles[0] ?? "");
  let password = $state("");
  let tried = $state(false);
  let busy = $state(false);
  let failure = $state<string | null>(null);

  const name = $derived(alias.trim() || hostname.trim());

  const errors = $derived.by(() => {
    const e: Partial<Record<"alias" | "hostname" | "user" | "port" | "key" | "password", string>> = {};
    if (!name) e.alias = "Enter a name.";
    else if (!/^[A-Za-z0-9._-]+$/.test(name)) e.alias = "Use letters, digits, “.”, “_” or “-”.";
    else if (name.length > 64) e.alias = "Use at most 64 characters.";
    else if (name !== initial?.alias && hosts.get(name)) e.alias = `“${name}” is already a server name.`;
    if (!hostname.trim()) e.hostname = "Enter a host name or IP address.";
    else if (/\s/.test(hostname.trim())) e.hostname = "No spaces in a host name.";
    if (/[\s@]/.test(user.trim())) e.user = "Just the user name, without “@”.";
    const p = Number(port);
    if (!Number.isInteger(p) || p < 1 || p > 65535) e.port = "1–65535";
    if (auth === "key" && !keyPath.trim()) e.key = "Choose the private key file.";
    if (auth === "password" && !initial?.hasSavedSecret && !password) e.password = "Enter the password.";
    return e;
  });
  const valid = $derived(Object.keys(errors).length === 0);

  async function chooseKey() {
    const home = await api.homeDir({ kind: "local" }).catch(() => null);
    const picked = await pickFile({ title: "Choose a private key", defaultPath: home ? `${home}/.ssh` : undefined });
    if (picked) keyPath = picked;
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    tried = true;
    failure = null;
    if (!valid || busy) return;
    const draft: HostDraft = {
      alias: name,
      hostname: hostname.trim(),
      user: user.trim() || null,
      port: Number(port),
      identityFile: auth === "key" ? keyPath.trim() : null,
      password: auth === "password" && password ? password : null,
    };
    busy = true;
    try {
      const saved = await api.saveHost(draft, initial?.alias ?? null);
      await hosts.reload().catch(() => {});
      ui.sheet = null;
      if (!initial) {
        void openHost(tabs.current, saved.alias);
      } else if (initial.alias !== saved.alias) {
        listings.dropSource({ kind: "ssh", host: initial.alias });
        // Renamed: tabs on the old name have nothing to reconnect to.
        for (const t of tabs.all) {
          if (t.location?.source.kind === "ssh" && t.location.source.host === initial.alias) openStart(t);
        }
      }
    } catch (err) {
      const v = toVfsError(err);
      failure = `${errorTitle(v.code)}: ${v.message}`;
    } finally {
      busy = false;
    }
  }

  const close = () => (ui.sheet = null);
</script>

<Sheet label={initial ? "Edit server" : "Add server"} oncancel={close} width={520}>
  <form class="form" onsubmit={submit} novalidate>
    <div class="head">
      <div class="glyph"><Server size={28} strokeWidth={1.6} /></div>
      <div>
        <h2>{initial ? `Edit “${initial.alias}”` : "Add Server"}</h2>
        <p>Connect over SSH. Saved in VPS Files, not in ~/.ssh/config.</p>
      </div>
    </div>

    <div class="grid">
      <label for="h-host">Server</label>
      <div class="pair">
        <input id="h-host" class="field mono" bind:value={hostname} placeholder="example.com or 203.0.113.10" spellcheck="false" autocomplete="off" />
        <label for="h-port" class="inline">Port</label>
        <input id="h-port" class="field mono port" bind:value={port} inputmode="numeric" autocomplete="off" />
      </div>
      {#if tried && (errors.hostname || errors.port)}<p class="err span">{errors.hostname ?? `Port: ${errors.port}`}</p>{/if}

      <label for="h-user">User</label>
      <input id="h-user" class="field" bind:value={user} placeholder="root" spellcheck="false" autocomplete="off" />
      {#if tried && errors.user}<p class="err span">{errors.user}</p>{/if}

      <label for="h-alias">Name</label>
      <input id="h-alias" class="field" bind:value={alias} placeholder={hostname.trim() || "my-server"} spellcheck="false" autocomplete="off" />
      {#if tried && errors.alias}<p class="err span">{errors.alias}</p>{:else}<p class="hint span">Shown in the sidebar. Letters, digits, “.”, “_”, “-”.</p>{/if}

      <span class="label">Sign in with</span>
      <div class="seg" role="radiogroup" aria-label="Sign in with">
        {#each AUTH as [value, label] (value)}
          <label class:on={auth === value}><input type="radio" name="auth" {value} bind:group={auth} />{label}</label>
        {/each}
      </div>

      {#if auth === "key"}
        <label for="h-key">Key file</label>
        <div class="pair">
          <input id="h-key" class="field mono" bind:value={keyPath} placeholder="~/.ssh/id_ed25519" spellcheck="false" autocomplete="off" />
          <button type="button" class="btn" onclick={chooseKey} disabled={!isTauri} title={isTauri ? undefined : "Available in the app"}>Choose…</button>
        </div>
        {#if tried && errors.key}<p class="err span">{errors.key}</p>{:else}<p class="hint span">If the key has a passphrase, you'll be asked for it once and can save it in the keychain.</p>{/if}
      {:else if auth === "password"}
        <label for="h-pass">Password</label>
        <input id="h-pass" class="field" type="password" bind:value={password} autocomplete="off" placeholder={initial?.hasSavedSecret ? "Leave empty to keep the saved one" : ""} />
        {#if tried && errors.password}<p class="err span">{errors.password}</p>{:else}<p class="hint span">Saved in the system keychain, never in a file.</p>{/if}
      {:else}
        <span></span>
        <p class="hint">Uses the keys loaded in ssh-agent, then the default ~/.ssh/id_* keys.</p>
      {/if}
    </div>

    {#if failure}<p class="err failure selectable">{failure}</p>{/if}

    <div class="buttons">
      <button type="button" class="btn" onclick={close}>Cancel</button>
      <button type="submit" class="btn primary" disabled={busy}>{busy ? "Saving…" : initial ? "Save" : "Add & Connect"}</button>
    </div>
  </form>
</Sheet>

<style>
  .form { display: flex; flex-direction: column; gap: 16px; }
  .head { display: flex; gap: 14px; align-items: center; }
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
  h2 { margin: 0 0 2px; font-size: var(--fs-ui); font-weight: 650; }
  p { margin: 0; color: var(--fg-secondary); font-size: var(--fs-secondary); }
  .grid {
    display: grid;
    grid-template-columns: 96px minmax(0, 1fr);
    align-items: center;
    gap: 8px 12px;
  }
  .grid > label, .label { justify-self: end; color: var(--fg-secondary); font-size: var(--fs-secondary); }
  .span { grid-column: 2; margin-top: -4px; }
  .hint { color: var(--fg-tertiary); font-size: var(--fs-small); }
  .err { color: var(--danger); font-size: var(--fs-small); }
  .failure { padding: 8px 10px; border-radius: var(--r-control); background: var(--bg-control); overflow-wrap: anywhere; }
  .field { width: 100%; min-width: 0; }
  .mono { font-family: var(--font-mono); font-size: var(--fs-secondary); }
  .pair { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .inline { flex: none; color: var(--fg-secondary); font-size: var(--fs-secondary); }
  .port { flex: none; width: 72px; }
  .pair .btn { flex: none; min-width: 0; }
  .seg { display: flex; padding: 2px; border-radius: 7px; background: var(--bg-control); }
  .seg label { flex: 1; text-align: center; padding: 4px 0; border-radius: 5px; }
  .seg label.on { background: var(--bg-window); box-shadow: var(--shadow-control); font-weight: 550; }
  .seg label:has(input:focus-visible) { outline: 2px solid var(--accent); outline-offset: -2px; }
  .seg input { position: absolute; opacity: 0; width: 0; height: 0; }
  .buttons { display: flex; justify-content: flex-end; gap: 8px; }
</style>
