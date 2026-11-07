// Persisted preferences (Preferences sheet, Mod+,) plus remembered UI state
// (sort, sidebar width).
// Per-viewer convenience only: storage may be unavailable, so every access
// is guarded and the defaults always work.

export type ViewMode = "icons" | "list" | "columns";
export type SortKey = "name" | "modified" | "size" | "kind";
export interface Sort { key: SortKey; asc: boolean }
export type Theme = "system" | "light" | "dark";

export interface SidebarSections { favourites: boolean; locations: boolean; servers: boolean }

interface Saved {
  /** View for new tabs and at launch. */
  view: ViewMode;
  sort: Sort;
  /** Hidden files shown by default (and in every open tab when changed). */
  showHidden: boolean;
  sidebarWidth: number;
  theme: Theme;
  sections: SidebarSections;
  /** Ask before deleting permanently (servers, Shift+Delete). */
  confirmDelete: boolean;
}

const KEY = "vpsfiles.prefs.v1";
const defaults: Saved = {
  view: "list",
  sort: { key: "name", asc: true },
  showHidden: false,
  sidebarWidth: 220,
  theme: "system",
  sections: { favourites: true, locations: true, servers: true },
  confirmDelete: true,
};

function load(): Saved {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? { ...defaults, ...(JSON.parse(raw) as Partial<Saved>) } : defaults;
  } catch {
    return defaults;
  }
}

class Prefs {
  #saved = load();
  view = $state<ViewMode>(this.#saved.view);
  sort = $state.raw<Sort>(this.#saved.sort);
  showHidden = $state(this.#saved.showHidden);
  sidebarWidth = $state(this.#saved.sidebarWidth);
  theme = $state<Theme>(this.#saved.theme);
  sections = $state.raw<SidebarSections>({ ...defaults.sections, ...this.#saved.sections });
  confirmDelete = $state(this.#saved.confirmDelete);

  constructor() {
    $effect.root(() => {
      $effect(() => {
        const data: Saved = {
          view: this.view, sort: this.sort, showHidden: this.showHidden,
          sidebarWidth: this.sidebarWidth, theme: this.theme,
          sections: this.sections, confirmDelete: this.confirmDelete,
        };
        try { localStorage.setItem(KEY, JSON.stringify(data)); } catch { /* storage unavailable */ }
      });
      $effect(() => {
        const root = document.documentElement;
        if (this.theme === "system") delete root.dataset.theme;
        else root.dataset.theme = this.theme;
      });
    });
  }
}

export const prefs = new Prefs();
