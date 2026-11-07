// Typed wrappers around every Tauri command. UI code imports only this
// module — never `invoke` directly — so the browser mock can stand in.
//
// Running `pnpm dev` in a plain browser uses ./mock.ts; inside Tauri the
// real Rust backend answers.

import type {
  DiskSpace, EditSession, Entry, EventMap, GitCommit, GitInfo, HostDraft, ServerInfo, HostInfo, JobSnapshot, Location, Place, SourceId,
  TextPreview, TransferRequest,
} from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface Api {
  listHosts(): Promise<HostInfo[]>;
  /** `interactive: false` never prompts (auto-connect / background): it
   * fails with code "cancelled" ("sign-in needed") instead. Default true. */
  connectHost(alias: string, interactive?: boolean): Promise<void>;
  disconnectHost(alias: string): Promise<void>;
  /** Create (previousAlias = null) or update an app-managed server. */
  saveHost(draft: HostDraft, previousAlias: string | null): Promise<HostInfo>;
  /** App-managed servers only; also deletes its saved password. */
  removeHost(alias: string): Promise<void>;
  /** Delete a password/passphrase saved in the OS keychain for this host. */
  forgetPassword(alias: string): Promise<void>;
  /** Path of ~/.ssh/config, to open it in an editor with openExternal. */
  sshConfigPath(): Promise<string>;
  /** Uptime, load, memory, OS of a connected server (one read-only exec). */
  serverInfo(alias: string): Promise<ServerInfo>;
  /** Open the OS terminal in this folder; on a server it runs ssh there. */
  openTerminal(location: Location): Promise<void>;

  /** Git state for a folder; null when it is not inside a repo or git is
   * not installed (locally, or on that server). */
  gitInfo(location: Location): Promise<GitInfo | null>;
  /** Newest-first history touching `location` (a folder or a file). */
  gitLog(location: Location, limit: number): Promise<GitCommit[]>;

  localPlaces(): Promise<Place[]>;
  homeDir(source: SourceId): Promise<string>;
  listDir(location: Location): Promise<Entry[]>;
  statPath(location: Location): Promise<Entry>;
  /** Free/total bytes of the volume holding `location`; null if unknown. */
  diskFree(location: Location): Promise<DiskSpace | null>;
  /** `location` is the full path of the folder to create. */
  createDir(location: Location): Promise<Entry>;
  renamePath(location: Location, newName: string): Promise<Entry>;
  /** Local + !permanent = move to trash. Remote is always permanent. */
  deletePaths(locations: Location[], permanent: boolean): Promise<void>;
  readText(location: Location, maxBytes: number): Promise<TextPreview>;
  /** Open with the OS default app. Local: opens in place, resolves null.
   * Remote: downloads a temp copy, opens it, uploads every save back, and
   * returns the session (updates arrive on "edit:update"). */
  openExternal(location: Location): Promise<EditSession | null>;
  editList(): Promise<EditSession[]>;
  /** Stop syncing and delete the temp copy. */
  editStop(id: string): Promise<void>;
  /** After a conflict: "upload" = overwrite the server copy with mine,
   * "discard" = drop my changes and re-download the server copy. */
  editResolve(id: string, action: "upload" | "discard"): Promise<void>;

  transferStart(request: TransferRequest): Promise<string>;
  transferList(): Promise<JobSnapshot[]>;
  transferPause(id: string): Promise<void>;
  transferResume(id: string): Promise<void>;
  transferCancel(id: string): Promise<void>;
  transferClearFinished(): Promise<void>;

  /** http://127.0.0.1 URL usable as <video>/<audio>/<img> src. */
  mediaUrl(location: Location): Promise<string>;
  /** "http://127.0.0.1:PORT/TOKEN" — lets media.ts build media and
   * thumbnail URLs synchronously (no IPC per row). */
  mediaBase(): Promise<string>;
  /** External players found on this machine, e.g. ["mpv", "vlc"]. */
  externalPlayers(): Promise<string[]>;
  /** Stream the file in an external player (first found, or `player`). */
  openInPlayer(location: Location, player: string | null): Promise<void>;

  /** `remember` saves the password/passphrase in the OS keychain. */
  promptRespond(id: string, accept: boolean, secret: string | null, remember?: boolean): Promise<void>;

  on<K extends keyof EventMap>(event: K, handler: (payload: EventMap[K]) => void): Promise<() => void>;
}

async function tauriApi(): Promise<Api> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  return {
    listHosts: () => invoke("list_hosts"),
    connectHost: (alias, interactive) => invoke("connect_host", { alias, interactive: interactive ?? true }),
    disconnectHost: (alias) => invoke("disconnect_host", { alias }),
    saveHost: (draft, previousAlias) => invoke("save_host", { draft, previousAlias }),
    removeHost: (alias) => invoke("remove_host", { alias }),
    forgetPassword: (alias) => invoke("forget_password", { alias }),
    sshConfigPath: () => invoke("ssh_config_path"),
    serverInfo: (alias) => invoke("server_info", { alias }),
    openTerminal: (location) => invoke("open_terminal", { location }),
    gitInfo: (location) => invoke("git_info", { location }),
    gitLog: (location, limit) => invoke("git_log", { location, limit }),

    localPlaces: () => invoke("local_places"),
    homeDir: (source) => invoke("home_dir", { source }),
    listDir: (location) => invoke("list_dir", { location }),
    statPath: (location) => invoke("stat_path", { location }),
    diskFree: (location) => invoke("disk_free", { location }),
    createDir: (location) => invoke("create_dir", { location }),
    renamePath: (location, newName) => invoke("rename_path", { location, newName }),
    deletePaths: (locations, permanent) => invoke("delete_paths", { locations, permanent }),
    readText: (location, maxBytes) => invoke("read_text", { location, maxBytes }),
    openExternal: (location) => invoke("open_external", { location }),
    editList: () => invoke("edit_list"),
    editStop: (id) => invoke("edit_stop", { id }),
    editResolve: (id, action) => invoke("edit_resolve", { id, action }),

    transferStart: (request) => invoke("transfer_start", { request }),
    transferList: () => invoke("transfer_list"),
    transferPause: (id) => invoke("transfer_pause", { id }),
    transferResume: (id) => invoke("transfer_resume", { id }),
    transferCancel: (id) => invoke("transfer_cancel", { id }),
    transferClearFinished: () => invoke("transfer_clear_finished"),

    mediaUrl: (location) => invoke("media_url", { location }),
    mediaBase: () => invoke("media_base"),
    externalPlayers: () => invoke("external_players"),
    openInPlayer: (location, player) => invoke("open_in_player", { location, player }),

    promptRespond: (id, accept, secret, remember) => invoke("prompt_respond", { id, accept, secret, remember: remember ?? false }),

    on: (event, handler) => listen(event, (e) => handler(e.payload as never)),
  };
}

export const api: Api = await (isTauri ? tauriApi() : import("./mock").then((m) => m.createMockApi()));
