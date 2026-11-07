# IPC contract

Rust side: `src-tauri/src/commands.rs` (+ types in `crates/*`).
TS side: `src/lib/ipc/types.ts`, `src/lib/ipc/api.ts`, `src/lib/ipc/mock.ts`.
**Change all of them in the same edit.** JS args are camelCase; Tauri maps
them to snake_case Rust params.

Every command rejects with `VfsError = { code, message }`.

## Commands

| Command | Args | Returns |
|---|---|---|
| `list_hosts` | — | `HostInfo[]` |
| `connect_host` | `alias` | — (may trigger `prompt:request`) |
| `disconnect_host` | `alias` | — |
| `save_host` | `draft: HostDraft, previousAlias` | `HostInfo` (app hosts only) |
| `remove_host` | `alias` | — (app hosts only) |
| `forget_password` | `alias` | — |
| `ssh_config_path` | — | `string` |
| `local_places` | — | `Place[]` |
| `home_dir` | `source: SourceId` | `string` |
| `list_dir` | `location` | `Entry[]` (unsorted) |
| `stat_path` | `location` | `Entry` |
| `disk_free` | `location` | `{ free, total }` or `null` |
| `create_dir` | `location` (full new path) | `Entry` |
| `rename_path` | `location, newName` | `Entry` (renamed) |
| `delete_paths` | `locations, permanent` | — (local + !permanent → trash) |
| `read_text` | `location, maxBytes` | `{ text, truncated }`, `unsupported` for binary |
| `open_external` | `location` | local: `null`; remote: `EditSession` (download → open → sync saves back) |
| `edit_list` | — | `EditSession[]` |
| `edit_stop` | `id` | — |
| `edit_resolve` | `id, action: "upload"\|"discard"` | — |
| `transfer_start` | `request: TransferRequest` | job id |
| `transfer_list` | — | `JobSnapshot[]` |
| `transfer_pause` / `_resume` / `_cancel` | `id` | — |
| `transfer_clear_finished` | — | — |
| `media_url` | `location` | `http://127.0.0.1:…` URL |
| `media_base` | — | `http://127.0.0.1:PORT/TOKEN` (see `src/lib/ipc/media.ts`) |
| `external_players` | — | `string[]` |
| `open_in_player` | `location, player` | — |
| `prompt_respond` | `id, accept, secret, remember` | — |

## Events

| Event | Payload |
|---|---|
| `conn:status` | `{ host, status: ConnStatus }` |
| `transfer:update` | `JobSnapshot` (on state change + ≤5×/s progress) |
| `prompt:request` | `PromptRequest` (`hostKey` / `password` / `passphrase`) |
| `edit:update` | `EditSession` |

## Shapes

```ts
SourceId      = { kind: "local" } | { kind: "ssh", host: string }
Location      = { source: SourceId, path: string }
Entry         = { name, path, kind: "file"|"dir"|"other", size, modified: ms|null,
                  mode: number|null, isSymlink, isHidden }
ConnStatus    = { state: "disconnected"|"connecting"|"connected" }
              | { state: "failed", error: VfsError }
HostInfo      = { alias, hostname, user, port, identityFiles, proxyJump,
                  forwardAgent, status: ConnStatus, origin: "sshConfig"|"app",
                  hasSavedSecret: boolean }
TransferRequest = { op: "copy"|"move", sources: Location[], destDir: Location,
                    conflict: "overwrite"|"merge"|"skip"|"keepBoth" }
                  // overwrite = Finder Replace (whole folder, staged swap)
                  // merge = folder into folder, clashing files replaced
JobSnapshot   = { id, op, state: "queued"|"scanning"|"running"|"paused"|
                  "completed"|"failed"|"cancelled", sources, destDir,
                  totalBytes, doneBytes, totalFiles, doneFiles, current,
                  bytesPerSec, error, createdAt, finishedAt }
DiskSpace     = { free: number, total: number }   // bytes
PromptRequest = { kind: "hostKey", id, host, algorithm, fingerprint }
              | { kind: "password", id, host, user }
              | { kind: "passphrase", id, host, keyPath }
```
