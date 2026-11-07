// Typed wrappers around every Tauri command. UI code imports only this
// module — never `invoke` directly — so the browser mock can stand in.
//
// Running `pnpm dev` in a plain browser uses ./mock.ts; inside Tauri the
// real Rust backend answers.

import type {
  DiskSpace, Entry, EventMap, HostInfo, JobSnapshot, Location, Place, SourceId,
  TextPreview, TransferRequest,
} from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface Api {
  listHosts(): Promise<HostInfo[]>;
  connectHost(alias: string): Promise<void>;
  disconnectHost(alias: string): Promise<void>;

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
  /** Open with the OS default app. Local only for now. */
  openExternal(location: Location): Promise<void>;

  transferStart(request: TransferRequest): Promise<string>;
  transferList(): Promise<JobSnapshot[]>;
  transferPause(id: string): Promise<void>;
  transferResume(id: string): Promise<void>;
  transferCancel(id: string): Promise<void>;
  transferClearFinished(): Promise<void>;

  /** http://127.0.0.1 URL usable as <video>/<audio>/<img> src. */
  mediaUrl(location: Location): Promise<string>;

  promptRespond(id: string, accept: boolean, secret: string | null): Promise<void>;

  on<K extends keyof EventMap>(event: K, handler: (payload: EventMap[K]) => void): Promise<() => void>;
}

async function tauriApi(): Promise<Api> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  return {
    listHosts: () => invoke("list_hosts"),
    connectHost: (alias) => invoke("connect_host", { alias }),
    disconnectHost: (alias) => invoke("disconnect_host", { alias }),

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

    transferStart: (request) => invoke("transfer_start", { request }),
    transferList: () => invoke("transfer_list"),
    transferPause: (id) => invoke("transfer_pause", { id }),
    transferResume: (id) => invoke("transfer_resume", { id }),
    transferCancel: (id) => invoke("transfer_cancel", { id }),
    transferClearFinished: () => invoke("transfer_clear_finished"),

    mediaUrl: (location) => invoke("media_url", { location }),

    promptRespond: (id, accept, secret) => invoke("prompt_respond", { id, accept, secret }),

    on: (event, handler) => listen(event, (e) => handler(e.payload as never)),
  };
}

export const api: Api = await (isTauri ? tauriApi() : import("./mock").then((m) => m.createMockApi()));
