// DEV-ONLY startup hooks, so a window that can be screenshotted but not
// clicked (Xvfb) can still be put into a given state. Read from the URL:
//   ?open=local:/home/me/Pictures | ?open=ssh:demo-web:/var/www
//   &view=icons|list|columns  &select=<name>  &quicklook=1  &prefs=1  &zoom=1.15
// App imports this module only when import.meta.env.DEV, so production
// builds don't contain it.

import { openPreferences } from "$lib/actions";
import type { Location } from "$lib/ipc/types";
import { hosts } from "$lib/stores/hosts.svelte";
import type { ViewMode } from "$lib/stores/prefs.svelte";
import { tabs } from "$lib/stores/tabs.svelte";
import { toasts } from "$lib/stores/toasts.svelte";
import { ui } from "$lib/stores/ui.svelte";
import { applyZoom } from "$lib/util/zoom";

/** "local:/p" → local; "ssh:alias:/p" → that server. */
function parseOpen(value: string): Location | null {
  const i = value.indexOf(":");
  if (i < 0) return null;
  const kind = value.slice(0, i);
  const rest = value.slice(i + 1);
  if (kind === "local") return rest ? { source: { kind: "local" }, path: rest } : null;
  if (kind !== "ssh") return null;
  const j = rest.indexOf(":");
  if (j <= 0 || j === rest.length - 1) return null;
  return { source: { kind: "ssh", host: rest.slice(0, j) }, path: rest.slice(j + 1) };
}

export async function runDevHooks() {
  if (!import.meta.env.DEV) return;
  const q = new URLSearchParams(location.search);
  if (![...q.keys()].length) return;
  console.info("[devhooks]", Object.fromEntries(q));

  // Not saved to prefs: a test run shouldn't change the user's size.
  const zoom = Number(q.get("zoom"));
  if (q.has("zoom") && zoom >= 0.5 && zoom <= 3) await applyZoom(zoom);

  const b = tabs.current;
  const view = q.get("view");
  if (view === "icons" || view === "list" || view === "columns") b.setView(view satisfies ViewMode);

  const open = q.get("open");
  if (open) {
    const loc = parseOpen(open);
    if (!loc) {
      console.warn("[devhooks] bad open=", open);
    } else {
      try {
        if (loc.source.kind === "ssh") await hosts.connect(loc.source.host);
        if (!(await b.go(loc))) console.warn("[devhooks] couldn't open", loc);
      } catch (e) {
        toasts.error(e, `[devhooks] couldn't open ${open}`);
      }
    }
  }

  const select = q.get("select");
  if (select) {
    const entry = b.items.find((e) => e.name === select);
    if (entry) b.selectOnly(entry.path);
    else console.warn("[devhooks] no entry named", select);
  }

  if (q.get("quicklook") === "1" && b.selection.length) ui.quickLook = true;
  if (q.get("prefs") === "1") openPreferences();
}
