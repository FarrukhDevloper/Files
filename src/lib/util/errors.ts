// VfsError → human text. Every rejected IPC call throws `{ code, message }`;
// anything else (a JS bug, a network glitch in the mock) becomes "other".

import type { ErrorCode, VfsError } from "$lib/ipc/types";

const titles: Record<ErrorCode, string> = {
  not_found: "Item not found",
  permission_denied: "Permission denied",
  already_exists: "An item with that name already exists",
  not_a_directory: "Not a folder",
  is_a_directory: "That's a folder",
  not_empty: "The folder isn't empty",
  invalid_name: "That name can't be used",
  not_connected: "Server not connected",
  auth: "Authentication failed",
  host_key: "Host key verification failed",
  sign_in_needed: "Sign-in needed",
  unsupported: "Not supported",
  cancelled: "Cancelled",
  io: "Input/output error",
  other: "Something went wrong",
};

const hints: Partial<Record<ErrorCode, string>> = {
  host_key:
    "The server's key doesn't match ~/.ssh/known_hosts. Someone could be intercepting the connection — check with the server's admin before removing the old key.",
  auth: "The server didn't accept the key, passphrase or password.",
  sign_in_needed: "Connect to it once to enter the password or confirm its key.",
  permission_denied: "You don't have access to this item.",
};

export function toVfsError(e: unknown): VfsError {
  if (e && typeof e === "object" && "code" in e && "message" in e && typeof (e as VfsError).code === "string") {
    const err = e as VfsError;
    return { code: err.code in titles ? err.code : "other", message: String(err.message) };
  }
  return { code: "other", message: e instanceof Error ? e.message : String(e) };
}

export function errorTitle(code: ErrorCode): string {
  return titles[code];
}

export function errorHint(err: VfsError): string {
  const hint = hints[err.code];
  return hint ? `${hint}\n${err.message}` : err.message;
}
