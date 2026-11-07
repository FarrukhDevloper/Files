<script lang="ts">
  import Sheet from "./Sheet.svelte";
  import { ICON_SIZES, prefs, type SidebarSections } from "$lib/stores/prefs.svelte";
  import { tabs } from "$lib/stores/tabs.svelte";
  import { ui } from "$lib/stores/ui.svelte";
  import { shortcutLabel } from "$lib/util/platform";
  import { SIZES } from "$lib/util/zoom";

  function setHidden(show: boolean) {
    prefs.showHidden = show;
    for (const t of tabs.all) t.setHidden(show);
  }

  function section(key: keyof SidebarSections, on: boolean) {
    prefs.sections = { ...prefs.sections, [key]: on };
  }

  const close = () => (ui.sheet = null);
</script>

<Sheet label="Preferences" oncancel={close} width={460}>
  <form class="prefs" onsubmit={(e) => { e.preventDefault(); close(); }}>
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
      <legend>Interface size</legend>
      <div class="seg" role="radiogroup" aria-label="Interface size">
        {#each SIZES as size (size.scale)}
          <label class:on={prefs.uiScale === size.scale}>
            <input type="radio" name="size" value={size.scale} bind:group={prefs.uiScale} />{size.label}
          </label>
        {/each}
      </div>
      <p class="hint">{shortcutLabel("Mod+=")} larger · {shortcutLabel("Mod+-")} smaller · {shortcutLabel("Mod+0")} default</p>
      <label class="check"><input type="checkbox" bind:checked={prefs.reduceMotion} /> Reduce motion</label>
    </fieldset>
    <div class="row">
      <fieldset>
        <legend>Thumbnails</legend>
        <div class="seg" role="radiogroup" aria-label="Thumbnails">
          {#each [["fill", "Fill"], ["fit", "Fit"]] as const as [value, label] (value)}
            <label class:on={prefs.thumbFit === value} title={value === "fill" ? "Uniform squares, cropped" : "Whole image, letterboxed"}>
              <input type="radio" name="thumbs" {value} bind:group={prefs.thumbFit} />{label}
            </label>
          {/each}
        </div>
      </fieldset>
      <fieldset>
        <legend>Icon size</legend>
        <div class="seg" role="radiogroup" aria-label="Icon size">
          {#each ICON_SIZES as s (s.size)}
            <label class:on={prefs.iconSize === s.size}>
              <input type="radio" name="icon-size" value={s.size} bind:group={prefs.iconSize} />{s.label}
            </label>
          {/each}
        </div>
      </fieldset>
    </div>
    <div class="row">
      <fieldset>
        <legend>New windows and tabs show</legend>
        <select class="field" bind:value={prefs.startPage} aria-label="Start page">
          <option value="home">Home</option>
          <option value="homeFolder">Home folder</option>
        </select>
      </fieldset>
      <fieldset>
        <legend>Folders open in</legend>
        <select class="field" bind:value={prefs.view} aria-label="Default view">
          <option value="icons">Icon view</option>
          <option value="list">List view</option>
          <option value="columns">Column view</option>
        </select>
      </fieldset>
    </div>
    <fieldset>
      <legend>Files</legend>
      <label class="check"><input type="checkbox" checked={prefs.showHidden} onchange={(e) => setHidden(e.currentTarget.checked)} /> Show hidden files</label>
      <label class="check"><input type="checkbox" bind:checked={prefs.confirmDelete} /> Ask before deleting permanently</label>
    </fieldset>
    <fieldset>
      <legend>Servers</legend>
      <label class="check"><input type="checkbox" bind:checked={prefs.autoConnect} /> Connect to servers at startup</label>
    </fieldset>
    <fieldset>
      <legend>Window</legend>
      <label class="check"><input type="checkbox" bind:checked={prefs.pathBar} /> Show path and status bar at the bottom</label>
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

<style>
  .prefs { display: flex; flex-direction: column; gap: 16px; }
  h2 { margin: 0; font-size: var(--fs-ui); font-weight: 650; }
  fieldset { display: flex; flex-direction: column; gap: 7px; min-width: 0; margin: 0; padding: 0; border: 0; }
  legend { padding: 0; margin-bottom: 7px; font-size: var(--fs-small); font-weight: 600; color: var(--fg-secondary); }
  .row { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
  .seg { display: flex; padding: 2px; border-radius: 7px; background: var(--bg-control); }
  .seg label { flex: 1; text-align: center; padding: 4px 0; border-radius: 5px; }
  .seg label.on { background: var(--bg-window); box-shadow: var(--shadow-control); font-weight: 550; }
  .seg label:has(input:focus-visible) { outline: 2px solid var(--accent); outline-offset: -2px; }
  .seg input { position: absolute; opacity: 0; width: 0; height: 0; }
  .hint { margin: 0; color: var(--fg-tertiary); font-size: var(--fs-small); }
  .field { width: 100%; }
  .check { display: flex; align-items: center; gap: 7px; font-size: var(--fs-secondary); }
  .buttons { display: flex; justify-content: flex-end; margin-top: 4px; }
</style>
