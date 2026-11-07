// Motion settings for JS-driven animation (Web Animations, Svelte
// transitions). CSS animations follow the same switch through
// `:root[data-motion="reduce"]` and prefers-reduced-motion (tokens.css).

import { prefs } from "$lib/stores/prefs.svelte";

const media = typeof matchMedia === "function" ? matchMedia("(prefers-reduced-motion: reduce)") : null;

/** Same curve as --ease-out in tokens.css. */
export const EASE = "cubic-bezier(0.2, 0.8, 0.2, 1)";

export function reducedMotion(): boolean {
  return prefs.reduceMotion || !!media?.matches;
}

/** `duration`, or 0 when motion is reduced. */
export function ms(duration: number): number {
  return reducedMotion() ? 0 : duration;
}

/** Svelte transition easing matching EASE (cubic ease-out). */
export function easeOut(t: number): number {
  return 1 - (1 - t) ** 3;
}
