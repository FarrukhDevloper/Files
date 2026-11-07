// Icons and labels for local places, shared by the sidebar and Home.

import type { Component } from "svelte";
import House from "@lucide/svelte/icons/house";
import Monitor from "@lucide/svelte/icons/monitor";
import FileText from "@lucide/svelte/icons/file-text";
import Download from "@lucide/svelte/icons/download";
import ImageIcon from "@lucide/svelte/icons/image";
import Music from "@lucide/svelte/icons/eighth-notes";
import Film from "@lucide/svelte/icons/film";
import Laptop from "@lucide/svelte/icons/laptop-minimal";
import HardDrive from "@lucide/svelte/icons/hard-drive";
import type { Place, PlaceKind } from "$lib/ipc/types";
import { baseName } from "$lib/util/path";

export { default as StartIcon } from "@lucide/svelte/icons/layout-dashboard";

export type IconComponent = Component<{ size?: number; strokeWidth?: number }>;

export const placeIcons: Record<PlaceKind, IconComponent> = {
  home: House, desktop: Monitor, documents: FileText, downloads: Download,
  pictures: ImageIcon, music: Music, videos: Film, root: Laptop, drive: HardDrive,
};

/** The home folder shows as the user's name, like Finder ("Home" is the
 *  start page, with its own icon: see StartIcon). */
export function placeLabel(p: Place): string {
  return p.kind === "home" ? baseName(p.path) || p.label : p.label;
}
