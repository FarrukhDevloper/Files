// IPC contract — mirrors the Rust types serialized by src-tauri.
// Source of truth: docs/IPC.md. Change both sides together.

export type SourceId = { kind: "local" } | { kind: "ssh"; host: string };

export interface Location {
  source: SourceId;
  /** Absolute, "/"-separated. Windows local: "C:/Users/me"; "/" = drive list. */
  path: string;
}

export type EntryKind = "file" | "dir" | "other";

export interface Entry {
  name: string;
  path: string;
  /** For symlinks: kind of the target. Broken links are "other". */
  kind: EntryKind;
  size: number;
  /** Unix epoch ms. */
  modified: number | null;
  /** Permission bits (st_mode & 0o7777). */
  mode: number | null;
  isSymlink: boolean;
  isHidden: boolean;
}

export interface DiskSpace {
  /** Bytes available to this user. */
  free: number;
  total: number;
}

export type PlaceKind =
  | "home" | "desktop" | "documents" | "downloads"
  | "pictures" | "music" | "videos" | "root" | "drive";

export interface Place {
  label: string;
  path: string;
  kind: PlaceKind;
}

export type ErrorCode =
  | "not_found" | "permission_denied" | "already_exists" | "not_a_directory"
  | "is_a_directory" | "not_empty" | "invalid_name" | "not_connected" | "auth"
  | "host_key" | "unsupported" | "cancelled" | "io" | "other";

/** What every rejected command throws. */
export interface VfsError {
  code: ErrorCode;
  message: string;
}

export type ConnStatus =
  | { state: "disconnected" }
  | { state: "connecting" }
  | { state: "connected" }
  | { state: "failed"; error: VfsError };

/** Where a host is defined. Only "app" hosts can be edited in the app. */
export type HostOrigin = "sshConfig" | "app";

export interface HostInfo {
  alias: string;
  hostname: string;
  user: string | null;
  port: number;
  identityFiles: string[];
  proxyJump: string | null;
  forwardAgent: boolean;
  status: ConnStatus;
  origin: HostOrigin;
  /** A password or key passphrase for this host is in the OS keychain. */
  hasSavedSecret: boolean;
}

/** Add/edit form for an app-managed server. */
export interface HostDraft {
  /** Unique name shown in the sidebar: letters, digits, ".", "_", "-". */
  alias: string;
  hostname: string;
  user: string | null;
  port: number;
  /** Local path of a private key; null = ssh-agent / default keys. */
  identityFile: string | null;
  /** Saved to the OS keychain when non-null. */
  password: string | null;
}

export interface ConnEvent {
  host: string;
  status: ConnStatus;
}

export type TransferOp = "copy" | "move";
/**
 * overwrite = Finder "Replace": the existing file or whole folder is replaced
 *             (staged + swapped, so a failed copy keeps the original).
 * merge     = folder onto folder: copy into it, replacing clashing files and
 *             keeping the rest; anything else behaves like overwrite.
 */
export type ConflictPolicy = "overwrite" | "merge" | "skip" | "keepBoth";
export type JobState =
  | "queued" | "scanning" | "running" | "paused"
  | "completed" | "failed" | "cancelled";

export interface TransferRequest {
  op: TransferOp;
  sources: Location[];
  destDir: Location;
  conflict: ConflictPolicy;
}

export interface JobSnapshot {
  id: string;
  op: TransferOp;
  state: JobState;
  sources: Location[];
  destDir: Location;
  totalBytes: number;
  doneBytes: number;
  totalFiles: number;
  doneFiles: number;
  current: string | null;
  bytesPerSec: number;
  error: VfsError | null;
  createdAt: number;
  finishedAt: number | null;
}

export interface TextPreview {
  text: string;
  /** File was longer than maxBytes, or not valid UTF-8 past some point. */
  truncated: boolean;
}

/** Opening a remote file in a local app: a temp copy is downloaded, opened,
 * and every save is uploaded back. */
export type EditState =
  | "downloading" | "open" | "uploading" | "synced" | "conflict" | "failed" | "closed";

export interface EditSession {
  id: string;
  location: Location;
  /** The local temp copy the OS app has open. */
  localPath: string;
  state: EditState;
  error: VfsError | null;
  /** Unix ms of the last successful upload. */
  lastSync: number | null;
}

/** Emitted on "prompt:request"; answer with api.promptRespond. */
export type PromptRequest =
  | { kind: "hostKey"; id: string; host: string; algorithm: string; fingerprint: string }
  | { kind: "password"; id: string; host: string; user: string }
  | { kind: "passphrase"; id: string; host: string; keyPath: string };

export interface EventMap {
  "conn:status": ConnEvent;
  "transfer:update": JobSnapshot;
  "prompt:request": PromptRequest;
  "edit:update": EditSession;
}
