// Path helpers mirroring crates/vfs/src/path.rs. Paths are absolute and
// "/"-separated; Windows local paths look like "C:/Users/me" and "/" is the
// virtual drive list. Never build paths from user-typed names here — only
// from names the backend returned or names the UI picked itself.

import type { Location, SourceId } from "$lib/ipc/types";

function isDrive(s: string): boolean {
  return s.length === 2 && /[A-Za-z]/.test(s[0]!) && s[1] === ":";
}

export function isRoot(path: string): boolean {
  return path === "/" || (path.length === 3 && isDrive(path.slice(0, 2)) && path.endsWith("/"));
}

/** Last component; "" for "/", "C:" for "C:/". */
export function baseName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1);
}

export function parentPath(path: string): string | null {
  if (path === "/") return null;
  const trimmed = path.replace(/\/+$/, "");
  if (isDrive(trimmed)) return "/";
  const i = trimmed.lastIndexOf("/");
  if (i <= 0) return "/";
  const p = trimmed.slice(0, i);
  return isDrive(p) ? `${p}/` : p;
}

export function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") ? dir + name : `${dir}/${name}`;
}

/** Every ancestor from the root down to `path` itself. */
export function ancestors(path: string): string[] {
  const out: string[] = [];
  for (let p: string | null = path; p !== null; p = parentPath(p)) out.push(p);
  return out.reverse();
}

export function sourceKey(s: SourceId): string {
  return s.kind === "local" ? "local" : `ssh:${s.host}`;
}

export function parseSourceKey(key: string): SourceId {
  return key.startsWith("ssh:") ? { kind: "ssh", host: key.slice(4) } : { kind: "local" };
}

export function sameSource(a: SourceId, b: SourceId): boolean {
  return sourceKey(a) === sourceKey(b);
}

export function sameLocation(a: Location | null, b: Location | null): boolean {
  return !!a && !!b && a.path === b.path && sameSource(a.source, b.source);
}

export function sourceLabel(s: SourceId): string {
  return s.kind === "local" ? "Computer" : s.host;
}
