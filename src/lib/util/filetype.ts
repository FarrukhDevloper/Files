// File-type detection by name: picks the icon and the "Kind" column text,
// and tells Quick Look which previewer to use.

import type { Entry } from "$lib/ipc/types";

export type IconType =
  | "folder" | "doc" | "text" | "code" | "image" | "video" | "audio"
  | "archive" | "pdf" | "disk";

export type PreviewKind = "image" | "video" | "audio" | "text" | "none";

interface TypeInfo { icon: IconType; kind: string }

const table: Record<string, TypeInfo> = {};
function add(icon: IconType, kinds: Record<string, string>) {
  for (const [exts, kind] of Object.entries(kinds)) {
    for (const ext of exts.split(" ")) table[ext] = { icon, kind };
  }
}

add("image", {
  png: "PNG image", "jpg jpeg": "JPEG image", gif: "GIF image", webp: "WebP image",
  svg: "SVG image", bmp: "BMP image", ico: "Icon", "heic heif": "HEIF image",
  avif: "AVIF image", "tif tiff": "TIFF image",
});
add("video", {
  "mp4 m4v": "MPEG-4 video", mkv: "Matroska video", webm: "WebM video",
  mov: "QuickTime movie", avi: "AVI video", wmv: "Windows Media video",
  flv: "Flash video", "mpg mpeg": "MPEG video", "3gp": "3GPP video",
});
add("audio", {
  mp3: "MP3 audio", flac: "FLAC audio", wav: "WAV audio", "ogg oga": "Ogg audio",
  opus: "Opus audio", m4a: "MPEG-4 audio", aac: "AAC audio", wma: "Windows Media audio",
});
add("archive", {
  zip: "ZIP archive", tar: "Tar archive", "gz tgz": "Gzip archive", "bz2 tbz2": "Bzip2 archive",
  "xz txz": "XZ archive", zst: "Zstandard archive", "7z": "7-Zip archive", rar: "RAR archive",
  deb: "Debian package", rpm: "RPM package", apk: "Android package", jar: "Java archive",
});
add("disk", {
  iso: "Disk image", img: "Disk image", dmg: "Disk image", qcow2: "QEMU disk image",
  vmdk: "VMware disk image", vdi: "VirtualBox disk image", "vhd vhdx": "Virtual hard disk",
});
add("pdf", { pdf: "PDF document" });
add("text", {
  txt: "Plain text", "md markdown": "Markdown", log: "Log file", csv: "CSV document",
  tsv: "TSV document", rtf: "Rich text", "srt vtt": "Subtitles",
});
add("code", {
  "js mjs cjs": "JavaScript", "ts mts cts": "TypeScript", jsx: "JavaScript (JSX)", tsx: "TypeScript (TSX)",
  svelte: "Svelte component", vue: "Vue component", rs: "Rust source", go: "Go source",
  py: "Python script", rb: "Ruby script", php: "PHP script", java: "Java source", kt: "Kotlin source",
  c: "C source", h: "C header", "cpp cc cxx": "C++ source", "hpp hh": "C++ header", cs: "C# source",
  swift: "Swift source", "sh bash zsh fish": "Shell script", ps1: "PowerShell script", lua: "Lua script",
  sql: "SQL", "html htm": "HTML document", css: "CSS stylesheet", "scss sass less": "Stylesheet",
  json: "JSON", "yaml yml": "YAML", toml: "TOML", xml: "XML document", "ini cfg conf": "Configuration",
  env: "Environment file", lock: "Lock file", service: "systemd unit", dockerfile: "Dockerfile",
  "pem crt key pub": "Key / certificate", diff: "Diff", patch: "Patch",
});
add("doc", {
  "doc docx odt": "Word document", "xls xlsx ods": "Spreadsheet", "ppt pptx odp": "Presentation",
  epub: "E-book", "ttf otf woff woff2": "Font", "exe msi": "Windows program", "appimage": "AppImage",
  "so dll dylib": "Library", wasm: "WebAssembly", "sqlite db": "Database", "bin dat": "Binary file",
});

/** Well-known extension-less or dotfile names. */
const byName: Record<string, TypeInfo> = {
  dockerfile: { icon: "code", kind: "Dockerfile" },
  makefile: { icon: "code", kind: "Makefile" },
  "cargo.lock": { icon: "code", kind: "Lock file" },
  readme: { icon: "text", kind: "Plain text" },
  license: { icon: "text", kind: "Plain text" },
  ".bashrc": { icon: "code", kind: "Shell script" },
  ".profile": { icon: "code", kind: "Shell script" },
  ".zshrc": { icon: "code", kind: "Shell script" },
  ".bash_profile": { icon: "code", kind: "Shell script" },
  ".bash_history": { icon: "text", kind: "Plain text" },
  ".gitignore": { icon: "code", kind: "Configuration" },
  ".env": { icon: "code", kind: "Environment file" },
  authorized_keys: { icon: "code", kind: "Key / certificate" },
  known_hosts: { icon: "code", kind: "Configuration" },
  config: { icon: "code", kind: "Configuration" },
};

/** "report.PDF" → "pdf". Only 1–8 letters/digits with at least one letter
 *  count, so "browser_dump.264923" or "v1.2" have no extension. */
export function extension(name: string): string {
  const i = name.lastIndexOf(".");
  if (i <= 0) return "";
  const ext = name.slice(i + 1).toLowerCase();
  return /^[a-z0-9]{1,8}$/.test(ext) && /[a-z]/.test(ext) ? ext : "";
}

function infoFor(name: string): TypeInfo | null {
  const lower = name.toLowerCase();
  const named = byName[lower];
  if (named) return named;
  if (lower.endsWith(".tar.gz") || lower.endsWith(".tar.xz") || lower.endsWith(".tar.zst") || lower.endsWith(".tar.bz2")) {
    return { icon: "archive", kind: "Tar archive" };
  }
  return table[extension(name)] ?? null;
}

export function iconFor(entry: Pick<Entry, "name" | "kind">): IconType {
  if (entry.kind === "dir") return "folder";
  return infoFor(entry.name)?.icon ?? "doc";
}

const kindCache = new Map<string, string>();

/** Text for the "Kind" column. */
export function kindLabel(entry: Pick<Entry, "name" | "kind" | "isSymlink">): string {
  if (entry.kind === "dir") return entry.isSymlink ? "Folder alias" : "Folder";
  if (entry.kind === "other") return entry.isSymlink ? "Broken alias" : "Special file";
  let label = kindCache.get(entry.name);
  if (label === undefined) {
    const ext = extension(entry.name);
    label = infoFor(entry.name)?.kind ?? (ext ? `${ext.toUpperCase()} file` : "Document");
    if (kindCache.size > 5000) kindCache.clear();
    kindCache.set(entry.name, label);
  }
  return entry.isSymlink ? `${label} alias` : label;
}

/** What Quick Look should try. "text" means "ask readText; it may say unsupported". */
export function previewKind(entry: Pick<Entry, "name" | "kind">): PreviewKind {
  if (entry.kind !== "file") return "none";
  const info = infoFor(entry.name);
  // Unknown names are often configs or logs: let the backend sniff them.
  if (!info) return "text";
  const ext = extension(entry.name);
  switch (info.icon) {
    case "image": return ext === "heic" || ext === "heif" || ext === "tif" || ext === "tiff" ? "none" : "image";
    case "video": return "video";
    case "audio": return "audio";
    case "text": case "code": return "text";
    default: return "none";
  }
}
