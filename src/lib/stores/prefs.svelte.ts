// Persisted preferences (Preferences sheet, Mod+,) plus remembered UI state
// (sort, sidebar width, interface size).
// Per-viewer convenience only: storage may be unavailable, so every access
// is guarded and the defaults always work.

import { applyZoom } from "$lib/util/zoom";

export type ViewMode = "icons" | "list" | "columns";
export type SortKey = "name" | "modified" | "size" | "kind";
export interface Sort { key: SortKey; asc: boolean }
export type Theme = "system" | "light" | "dark";

export interface SidebarSections { favourites: boolean; locations: boolean; servers: boolean }
/** What new windows and tabs show first. */
export type StartPage = "home" | "homeFolder";


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
  /** Interface size: 0.9 / 1 / 1.15 / 1.3 (see util/zoom.ts). */
  uiScale: number;
  /** Bottom path + status bar (the address bar is in the toolbar). */
  pathBar: boolean;
  startPage: StartPage;
  /** Connect to every server when the app starts. */
  autoConnect: boolean;
  /** Fewer animations (also follows the OS setting). */
  reduceMotion: boolean;
  /** List view: a "Git" column inside repositories. */
  gitColumn: boolean;
}

const KEY = "vpsfiles.prefs.v1";
const defaults: Saved = {
  view: "list",
  sort: { key: "name", asc: true },
  showHidden: false,
  sidebarWidth: 230,
  theme: "system",
  sections: { favourites: true, locations: true, servers: true },
  confirmDelete: true,
  uiScale: 1,
  pathBar: false,
  startPage: "home",
  autoConnect: true,
  reduceMotion: false,
  gitColumn: false,
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
  uiScale = $state(this.#saved.uiScale);
  pathBar = $state(this.#saved.pathBar);
  startPage = $state<StartPage>(this.#saved.startPage);
  autoConnect = $state(this.#saved.autoConnect);
  reduceMotion = $state(this.#saved.reduceMotion);
  gitColumn = $state(this.#saved.gitColumn);

  constructor() {
    $effect.root(() => {
      $effect(() => {
        const data: Saved = {
          view: this.view, sort: this.sort, showHidden: this.showHidden,
          sidebarWidth: this.sidebarWidth, theme: this.theme,
          sections: this.sections, confirmDelete: this.confirmDelete,
          uiScale: this.uiScale, pathBar: this.pathBar, startPage: this.startPage,
          autoConnect: this.autoConnect, reduceMotion: this.reduceMotion, gitColumn: this.gitColumn,
        };
        try { localStorage.setItem(KEY, JSON.stringify(data)); } catch { /* storage unavailable */ }
      });
      $effect(() => {
        const root = document.documentElement;
        if (this.theme === "system") delete root.dataset.theme;
        else root.dataset.theme = this.theme;
      });
      $effect(() => {
        const root = document.documentElement;
        if (this.reduceMotion) root.dataset.motion = "reduce";
        else delete root.dataset.motion;
      });
      $effect(() => {
        void applyZoom(this.uiScale).catch(() => { /* zoom unsupported: stay at 100 % */ });
      });
    });
  }
}

export const prefs = new Prefs();
