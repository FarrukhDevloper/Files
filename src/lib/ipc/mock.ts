// In-memory stand-in for the Rust backend, used when the UI runs in a plain
// browser (`pnpm dev`). It honours the same contract as api.ts so screens can
// be built and demoed before the backend exists. Keep it boring and faithful:
// errors use real codes, remote calls have latency, transfers emit progress.

import type { Api } from "./api";
import type {
  ConnStatus, EditSession, EditState, Entry, EventMap, GitCommit, GitFileStatus, GitInfo, HostInfo, JobSnapshot,
  Location, Place, ServerInfo,
  PromptRequest, SourceId, VfsError,
} from "./types";

type Node = { kind: "dir"; children: Map<string, Node>; modified: number }
  | { kind: "file"; size: number; modified: number };

const now = Date.now();
const day = 86_400_000;

function dir(children: Record<string, Node>, age = 3): Node {
  return { kind: "dir", children: new Map(Object.entries(children)), modified: now - age * day };
}
function file(size: number, age = 1): Node {
  return { kind: "file", size, modified: now - age * day };
}

function sourceKey(s: SourceId): string {
  return s.kind === "local" ? "local" : `ssh:${s.host}`;
}

const trees = new Map<string, Node>([
  ["local", dir({
    home: dir({
      me: dir({
        Desktop: dir({ "todo.md": file(1_204), "screenshot.png": file(842_133) }),
        Documents: dir({
          "invoice-2026-10.pdf": file(98_220, 9),
          "notes.txt": file(5_311, 2),
          // A git repository (see mockGit below).
          Projects: dir({ "vps-files": dir({
            "README.md": file(2_048), "package.json": file(1_204, 9), ".gitignore": file(96, 40), "notes.md": file(512, 0),
            src: dir({ "main.ts": file(3_200, 6), "new-view.svelte": file(4_310, 0), lib: dir({ "api.ts": file(5_100, 0), "path.ts": file(2_200, 12) }) }),
            docs: dir({ "DESIGN.md": file(6_000, 1), "IPC.md": file(9_800, 3) }),
          }) }),
          // Exercises list virtualization: 10k entries must scroll at 60 fps.
          "10k files": dir(Object.fromEntries(Array.from({ length: 10_000 }, (_, i) =>
            [`log-${String(i).padStart(5, "0")}.txt`, file(1_000 + ((i * 7919) % 900_000), i % 400)]))),
        }),
        Downloads: dir({
          "ubuntu-26.04-live-server-amd64.iso": file(3_221_225_472, 20),
          "song.mp3": file(7_340_032, 4),
          "trailer.mp4": file(52_428_800, 6),
        }),
        Pictures: dir({
          "sunset.jpg": file(2_621_440, 30), "logo.svg": file(4_096, 60),
          // Exercises lazy thumbnails: only visible ones may load, 4 at a time.
          Gallery: dir(Object.fromEntries(Array.from({ length: 240 }, (_, i) =>
            [`photo-${String(i).padStart(3, "0")}.jpg`, file(200_000 + i * 1_000, i % 30)]))),
        }),
        Music: dir({}),
        Videos: dir({ "talk.webm": file(184_549_376, 12) }),
        ".bashrc": file(3_771, 200),
      }),
    }),
  })],
  ["ssh:demo-web", dir({
    root: dir({}),
    var: dir({ www: dir({ app: dir({
      "index.html": file(14_203), "app.js": file(512_000), assets: dir({ "hero.webp": file(301_000) }),
    }) }) }),
    etc: dir({ nginx: dir({ "nginx.conf": file(2_656, 90) }) }),
    home: dir({ deploy: dir({ "backup-2026-11-01.tar.gz": file(1_610_612_736, 6), logs: dir({}) }) }),
  })],
  ["ssh:demo-db", dir({
    root: dir({}),
    home: dir({ admin: dir({ dumps: dir({ "prod.sql.gz": file(734_003_200, 1) }) }) }),
    srv: dir({ media: dir({ "lecture-01.mp4": file(901_775_360, 15) }) }),
  })],
]);

const hosts: (Omit<HostInfo, "status" | "origin" | "hasSavedSecret"> & { origin?: HostInfo["origin"] })[] = [
  // Added in the app (editable): key login, no prompts.
  { alias: "lab", hostname: "198.51.100.7", user: "dev", port: 22, identityFiles: ["~/.ssh/lab_ed25519"], proxyJump: null, forwardAgent: false, origin: "app" },
  { alias: "demo-web", hostname: "203.0.113.10", user: "deploy", port: 22, identityFiles: ["~/.ssh/id_ed25519"], proxyJump: null, forwardAgent: false },
  // Unknown host key on first connect → hostKey prompt.
  { alias: "demo-db", hostname: "203.0.113.20", user: "admin", port: 2222, identityFiles: [], proxyJump: null, forwardAgent: false },
  // No usable key → password prompt (the mock password is "demo").
  { alias: "demo-pass", hostname: "203.0.113.30", user: "root", port: 22, identityFiles: [], proxyJump: null, forwardAgent: false },
  // Unreachable → connect fails.
  { alias: "demo-down", hostname: "203.0.113.40", user: "ops", port: 22, identityFiles: ["~/.ssh/id_rsa"], proxyJump: null, forwardAgent: false },
];
trees.set("ssh:demo-pass", dir({ root: dir({ "notes.txt": file(812) }), etc: dir({ hostname: file(12) }) }));
const homes: Record<string, string> = { local: "/home/me", "ssh:demo-web": "/home/deploy", "ssh:demo-db": "/home/admin", "ssh:demo-pass": "/root" };
const trustedHosts = new Set(["demo-web", "demo-pass", "demo-down", "lab"]);
type Answer = { accept: boolean; secret: string | null; remember: boolean };
const pendingPrompts = new Map<string, (answer: Answer) => void>();
/** The "OS keychain": passwords saved per host alias. */
const keychain = new Map<string, string>();

/** A small server filesystem for hosts added in the app. */
function serverTree(user: string): Node {
  const home = dir({ "README.txt": file(1_420), projects: dir({ api: dir({ "main.go": file(8_201) }) }), ".bashrc": file(3_771, 90) });
  return dir({
    root: user === "root" ? home : dir({}),
    home: user === "root" ? dir({}) : dir({ [user]: home }),
    etc: dir({ hostname: file(9), "os-release": file(386, 120) }),
    var: dir({ log: dir({ "syslog": file(1_048_576, 0), "auth.log": file(204_800, 0) }) }),
  });
}

function addServerFiles(alias: string, user: string | null) {
  const u = user ?? "root";
  trees.set(`ssh:${alias}`, serverTree(u));
  homes[`ssh:${alias}`] = u === "root" ? "/root" : `/home/${u}`;
}
addServerFiles("lab", "dev");

/** demo-pass wants "demo"; app hosts without a key take any password. */
function needsPassword(alias: string): boolean {
  const h = hosts.find((x) => x.alias === alias);
  return alias === "demo-pass" || (h?.origin === "app" && h.identityFiles.length === 0);
}
function passwordOk(alias: string, secret: string | null): boolean {
  return alias === "demo-pass" ? secret === "demo" : !!secret;
}

function ask(request: PromptRequest): Promise<Answer> {
  return new Promise((resolve) => {
    pendingPrompts.set(request.id, resolve);
    emit("prompt:request", request);
  });
}
const status = new Map<string, ConnStatus>(hosts.map((h) => [h.alias, { state: "disconnected" }]));

type Handler = (payload: unknown) => void;
const listeners = new Map<string, Set<Handler>>();
function emit<K extends keyof EventMap>(event: K, payload: EventMap[K]) {
  listeners.get(event)?.forEach((h) => h(payload));
}

function fail(code: VfsError["code"], message: string): never {
  throw { code, message } satisfies VfsError;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function latency(s: SourceId) {
  await sleep(s.kind === "local" ? 5 : 60 + Math.random() * 180);
}

function requireConnected(s: SourceId) {
  if (s.kind === "ssh" && status.get(s.host)?.state !== "connected") {
    fail("not_connected", `${s.host} is not connected`);
  }
}

function split(path: string): string[] {
  return path.split("/").filter(Boolean);
}

function lookup(loc: Location): Node {
  let node = trees.get(sourceKey(loc.source));
  if (!node) fail("not_found", sourceKey(loc.source));
  for (const part of split(loc.path)) {
    if (node.kind !== "dir") fail("not_a_directory", loc.path);
    const next = node.children.get(part);
    if (!next) fail("not_found", loc.path);
    node = next;
  }
  return node;
}

function parentOf(loc: Location): { dir: Extract<Node, { kind: "dir" }>; name: string } {
  const parts = split(loc.path);
  const name = parts.pop();
  if (!name) fail("invalid_name", loc.path);
  const parent = lookup({ ...loc, path: "/" + parts.join("/") });
  if (parent.kind !== "dir") fail("not_a_directory", loc.path);
  return { dir: parent, name };
}

function toEntry(name: string, path: string, node: Node): Entry {
  return {
    name, path, kind: node.kind,
    size: node.kind === "file" ? node.size : 0,
    modified: node.modified,
    mode: node.kind === "dir" ? 0o755 : 0o644,
    isSymlink: false,
    isHidden: name.startsWith("."),
  };
}

function join(dirPath: string, name: string) {
  return dirPath.endsWith("/") ? dirPath + name : `${dirPath}/${name}`;
}

function totalSize(node: Node): { bytes: number; files: number } {
  if (node.kind === "file") return { bytes: node.size, files: 1 };
  let bytes = 0, files = 0;
  for (const c of node.children.values()) {
    const t = totalSize(c);
    bytes += t.bytes; files += t.files;
  }
  return { bytes, files };
}

function clone(node: Node): Node {
  return node.kind === "file"
    ? { ...node, modified: Date.now() }
    : { kind: "dir", modified: Date.now(), children: new Map([...node.children].map(([k, v]) => [k, clone(v)])) };
}

const jobs = new Map<string, JobSnapshot>();
const jobTimers = new Map<string, ReturnType<typeof setInterval>>();

function runJob(id: string, onDone: () => void) {
  const job = jobs.get(id)!;
  // Remote↔remote is "direct" and fast; anything touching local is slower.
  const remoteToRemote = job.sources.every((s) => s.source.kind === "ssh") && job.destDir.source.kind === "ssh";
  const speed = remoteToRemote ? 180_000_000 : 25_000_000;
  const timer = setInterval(() => {
    const j = jobs.get(id)!;
    if (j.state !== "running") return;
    const step = Math.min(speed / 5, j.totalBytes - j.doneBytes);
    const doneBytes = j.doneBytes + step;
    const finished = doneBytes >= j.totalBytes;
    const next: JobSnapshot = {
      ...j, doneBytes,
      doneFiles: finished ? j.totalFiles : Math.floor((doneBytes / Math.max(j.totalBytes, 1)) * j.totalFiles),
      bytesPerSec: speed * (0.85 + Math.random() * 0.3),
      state: finished ? "completed" : "running",
      current: finished ? null : j.sources[0]?.path ?? null,
      finishedAt: finished ? Date.now() : null,
    };
    jobs.set(id, next);
    emit("transfer:update", next);
    if (finished) {
      clearInterval(timer);
      jobTimers.delete(id);
      onDone();
    }
  }, 200);
  jobTimers.set(id, timer);
}

function setJobState(id: string, state: JobSnapshot["state"]) {
  const j = jobs.get(id);
  if (!j) fail("not_found", id);
  const next = { ...j, state, bytesPerSec: 0, finishedAt: state === "cancelled" ? Date.now() : j.finishedAt };
  jobs.set(id, next);
  emit("transfer:update", next);
}

// ---- git: one local repository with local changes and history ----

const REPO = "/home/me/Documents/Projects/vps-files";
/** Repo-relative path → status (what `git status --porcelain=v2` would say). */
const repoChanges: Record<string, GitFileStatus> = {
  "README.md": "modified",
  "src/lib/api.ts": "modified",
  "src/new-view.svelte": "added",
  "docs/DESIGN.md": "renamed",
  "notes.md": "untracked",
};
const STRENGTH: GitFileStatus[] = ["untracked", "added", "renamed", "modified", "deleted", "conflicted"];

function mockGitInfo(loc: Location): GitInfo | null {
  if (loc.source.kind !== "local" || !(loc.path === REPO || loc.path.startsWith(`${REPO}/`))) return null;
  const rel = loc.path.slice(REPO.length + 1);
  const entries: Record<string, GitFileStatus> = {};
  for (const [path, status] of Object.entries(repoChanges)) {
    if (rel && !path.startsWith(`${rel}/`)) continue;
    const name = (rel ? path.slice(rel.length + 1) : path).split("/")[0]!;
    const had = entries[name];
    // A folder shows the strongest status of anything below it.
    if (!had || STRENGTH.indexOf(status) > STRENGTH.indexOf(had)) entries[name] = status;
  }
  return {
    root: REPO, branch: "main", head: "3f9c2ab", upstream: "origin/main", ahead: 2, behind: 1,
    remoteUrl: "git@github.com:example/vps-files.git", webUrl: "https://github.com/example/vps-files",
    changed: Object.keys(repoChanges).length, entries,
  };
}

const SUBJECTS = [
  "Address bar: fold leading crumbs into a menu", "Thumbnails: separate local/remote gates",
  "Listing cache with stale-while-revalidate", "Fix: late connect must not navigate",
  "Quiet auto-connect at startup", "Home page with server cards", "Add/Edit server sheet",
  "Open remote files in local apps", "Edit sessions: conflict sheet", "Dual pane (F3)",
  "Animation pass: transform/opacity only", "Quick Look zooms from the item", "Optimistic rename and delete",
  "Column view reads the shared cache", "Bump russh to 0.54", "SFTP: pipeline READDIR",
  "Media server: thumbnail route", "Keychain: remember passwords", "ProxyJump support",
  "Transfers: merge conflict policy", "Preferences sheet", "Keyboard audit", "Tab strip",
  "Drag & drop between folders", "Miller columns", "Conflict sheet for name clashes",
  "List view: sticky header", "Icon view virtualization", "Initial architecture", "Initial commit",
];

function mockGitLog(loc: Location, limit: number): GitCommit[] {
  if (!mockGitInfo(loc)) return [];
  // A file has a shorter history than its repository.
  const step = loc.path === REPO || lookupKind(loc) === "dir" ? 1 : 3;
  const out: GitCommit[] = [];
  for (let i = 0; i < SUBJECTS.length && out.length < limit; i += step) {
    const hash = Array.from({ length: 40 }, (_, k) => "0123456789abcdef"[(i * 7 + k * 13 + 3) % 16]).join("");
    const refs = i === 0 ? ["HEAD -> main"] : i === 2 ? ["origin/main"] : i === 9 ? ["tag: v0.2.0"] : i === 22 ? ["tag: v0.1.0"] : [];
    out.push({
      hash, shortHash: hash.slice(0, 7), author: i % 3 === 1 ? "CI Bot" : "Farrukh", email: "dev@example.com",
      date: now - i * 9 * 3_600_000 - 1_800_000, subject: SUBJECTS[i]!, refs,
    });
  }
  return out;
}

function lookupKind(loc: Location): Node["kind"] | null {
  try { return lookup(loc).kind; } catch { return null; }
}

/** Plausible health numbers, different per server. */
function mockServerInfo(alias: string): ServerInfo {
  const n = [...alias].reduce((a, c) => a + c.charCodeAt(0), 0);
  const gb = 1024 ** 3;
  const total = [4, 8, 16, 32][n % 4]! * gb;
  return {
    os: n % 2 ? "Ubuntu 24.04.1 LTS" : "Debian GNU/Linux 12 (bookworm)",
    kernel: n % 2 ? "6.8.0-45-generic" : "6.1.0-25-amd64",
    uptimeSecs: 3_600 * (20 + (n * 37) % 900),
    load: [((n % 9) + 1) / 10, ((n % 7) + 1) / 10, ((n % 5) + 1) / 10],
    cpus: [2, 4, 8][n % 3]!,
    memTotal: total,
    memAvailable: total * (0.25 + ((n % 50) / 100)),
  };
}

// ---- edit sessions: a remote file "open in its app" ----
// downloading → open → (a save) uploading → synced. On demo-db somebody else
// changes the file meanwhile, so the save ends in "conflict".

const edits = new Map<string, EditSession>();
const editTimers = new Map<string, ReturnType<typeof setTimeout>[]>();
const CONFLICT_HOST = "demo-db";

function editSet(id: string, state: EditState, patch: Partial<EditSession> = {}) {
  const s = edits.get(id);
  if (!s) return;
  const next = { ...s, ...patch, state };
  if (state === "closed") edits.delete(id);
  else edits.set(id, next);
  emit("edit:update", next);
}

/** Run `steps` one after another; Stop cancels what's left. */
function editPlan(id: string, steps: [number, () => void][]) {
  for (const t of editTimers.get(id) ?? []) clearTimeout(t);
  let at = 0;
  editTimers.set(id, steps.map(([delay, fn]) => setTimeout(fn, (at += delay))));
}

function touchRemote(loc: Location) {
  const node = lookup(loc);
  if (node.kind === "file") node.modified = Date.now();
}

export function createMockApi(): Api {
  return {
    async listHosts() {
      return hosts.map((h) => ({ ...h, origin: h.origin ?? "sshConfig", hasSavedSecret: keychain.has(h.alias), status: status.get(h.alias)! }));
    },
    async connectHost(alias, interactive = true) {
      if (!status.has(alias)) fail("not_found", alias);
      status.set(alias, { state: "connecting" });
      emit("conn:status", { host: alias, status: { state: "connecting" } });
      await sleep(700);
      const failWith = (error: VfsError): never => {
        status.set(alias, { state: "failed", error });
        emit("conn:status", { host: alias, status: { state: "failed", error } });
        throw error;
      };
      if (alias === "demo-down") failWith({ code: "io", message: "connect to 203.0.113.40 port 22: Connection timed out" });
      // Not interactive (startup / background): never prompt, just report.
      const signIn = (what: string): never => failWith({ code: "sign_in_needed", message: `sign-in needed: ${what}` });
      if (!trustedHosts.has(alias)) {
        if (!interactive) signIn("unknown host key");
        const answer = await ask({
          kind: "hostKey", id: crypto.randomUUID(), host: alias, algorithm: "ED25519",
          fingerprint: "SHA256:q3Vn1dHkPZ0rM9yQe5o8wXkz7cB2fTgLjA4sU6vN1hE",
        });
        if (!answer.accept) failWith({ code: "cancelled", message: "Host key not trusted" });
        trustedHosts.add(alias);
      }
      if (needsPassword(alias)) {
        const saved = keychain.get(alias);
        if (saved !== undefined && !passwordOk(alias, saved)) keychain.delete(alias); // stale: ask again
        if (!keychain.has(alias)) {
          if (!interactive) signIn("password");
          const user = hosts.find((h) => h.alias === alias)?.user ?? "root";
          const answer = await ask({ kind: "password", id: crypto.randomUUID(), host: alias, user });
          if (!answer.accept) failWith({ code: "cancelled", message: "Password prompt cancelled" });
          if (!passwordOk(alias, answer.secret)) failWith({ code: "auth", message: "Permission denied (publickey,password)" });
          if (answer.remember) keychain.set(alias, answer.secret!);
        }
      }
      status.set(alias, { state: "connected" });
      emit("conn:status", { host: alias, status: { state: "connected" } });
    },
    async disconnectHost(alias) {
      status.set(alias, { state: "disconnected" });
      emit("conn:status", { host: alias, status: { state: "disconnected" } });
    },
    async saveHost(draft, previousAlias) {
      await sleep(150);
      if (!/^[A-Za-z0-9._-]{1,64}$/.test(draft.alias)) fail("invalid_name", draft.alias);
      if (!draft.hostname.trim()) fail("invalid_name", "empty host name");
      const i = previousAlias !== null ? hosts.findIndex((h) => h.alias === previousAlias) : -1;
      if (previousAlias !== null && i < 0) fail("not_found", previousAlias);
      if (i >= 0 && hosts[i]!.origin !== "app") fail("unsupported", `${previousAlias} is defined in ~/.ssh/config`);
      if (draft.alias !== previousAlias && hosts.some((h) => h.alias === draft.alias)) fail("already_exists", draft.alias);
      const host = {
        alias: draft.alias, hostname: draft.hostname, user: draft.user, port: draft.port,
        identityFiles: draft.identityFile ? [draft.identityFile] : [], proxyJump: null,
        forwardAgent: false, origin: "app" as const,
      };
      if (i >= 0) {
        // Editing drops the connection; a rename carries files and secrets over.
        const old = previousAlias!;
        if (status.get(old)?.state !== "disconnected") emit("conn:status", { host: old, status: { state: "disconnected" } });
        status.delete(old);
        if (old !== draft.alias) {
          trees.set(`ssh:${draft.alias}`, trees.get(`ssh:${old}`)!);
          trees.delete(`ssh:${old}`);
          homes[`ssh:${draft.alias}`] = homes[`ssh:${old}`]!;
          const secret = keychain.get(old);
          keychain.delete(old);
          if (secret !== undefined) keychain.set(draft.alias, secret);
        }
        hosts[i] = host;
      } else {
        hosts.push(host);
        addServerFiles(draft.alias, draft.user);
      }
      status.set(draft.alias, { state: "disconnected" });
      if (draft.password) keychain.set(draft.alias, draft.password);
      return { ...host, hasSavedSecret: keychain.has(draft.alias), status: status.get(draft.alias)! };
    },
    async removeHost(alias) {
      const i = hosts.findIndex((h) => h.alias === alias);
      if (i < 0) fail("not_found", alias);
      if (hosts[i]!.origin !== "app") fail("unsupported", `${alias} is defined in ~/.ssh/config`);
      if (status.get(alias)?.state !== "disconnected") emit("conn:status", { host: alias, status: { state: "disconnected" } });
      hosts.splice(i, 1);
      status.delete(alias);
      keychain.delete(alias);
      trees.delete(`ssh:${alias}`);
    },
    async forgetPassword(alias) {
      if (!status.has(alias)) fail("not_found", alias);
      keychain.delete(alias);
    },
    // Lead stubs for round 3 — the ui agent turns them into realistic data.
    async serverInfo(alias) {
      requireConnected({ kind: "ssh", host: alias });
      await latency({ kind: "ssh", host: alias });
      return mockServerInfo(alias);
    },
    async openTerminal(loc) {
      requireConnected(loc.source);
      console.info("[mock] open terminal:", loc);
    },
    async gitInfo(loc) {
      requireConnected(loc.source);
      await latency(loc.source);
      return mockGitInfo(loc);
    },
    async gitLog(loc, limit) {
      requireConnected(loc.source);
      await sleep(loc.source.kind === "local" ? 40 : 250);
      return mockGitLog(loc, limit);
    },
    async sshConfigPath() {
      return "/home/me/.ssh/config";
    },

    async localPlaces(): Promise<Place[]> {
      return [
        { label: "Home", path: "/home/me", kind: "home" },
        { label: "Desktop", path: "/home/me/Desktop", kind: "desktop" },
        { label: "Documents", path: "/home/me/Documents", kind: "documents" },
        { label: "Downloads", path: "/home/me/Downloads", kind: "downloads" },
        { label: "Pictures", path: "/home/me/Pictures", kind: "pictures" },
        { label: "Music", path: "/home/me/Music", kind: "music" },
        { label: "Videos", path: "/home/me/Videos", kind: "videos" },
        { label: "Computer", path: "/", kind: "root" },
      ];
    },
    async homeDir(source) {
      requireConnected(source);
      return homes[sourceKey(source)] ?? "/";
    },
    async listDir(loc) {
      requireConnected(loc.source);
      await latency(loc.source);
      const node = lookup(loc);
      if (node.kind !== "dir") fail("not_a_directory", loc.path);
      return [...node.children].map(([name, n]) => toEntry(name, join(loc.path, name), n));
    },
    async statPath(loc) {
      requireConnected(loc.source);
      await latency(loc.source);
      const node = lookup(loc);
      return toEntry(split(loc.path).at(-1) ?? "", loc.path, node);
    },
    async createDir(loc) {
      requireConnected(loc.source);
      await latency(loc.source);
      const { dir: parent, name } = parentOf(loc);
      if (parent.children.has(name)) fail("already_exists", loc.path);
      const node = dir({}, 0);
      parent.children.set(name, node);
      return toEntry(name, loc.path, node);
    },
    async renamePath(loc, newName) {
      requireConnected(loc.source);
      if (!newName || newName.includes("/") || newName === "." || newName === "..") fail("invalid_name", newName);
      await latency(loc.source);
      const { dir: parent, name } = parentOf(loc);
      const node = parent.children.get(name);
      if (!node) fail("not_found", loc.path);
      if (parent.children.has(newName)) fail("already_exists", newName);
      parent.children.delete(name);
      parent.children.set(newName, node);
      const parentPath = "/" + split(loc.path).slice(0, -1).join("/");
      return toEntry(newName, join(parentPath, newName), node);
    },
    async deletePaths(locs) {
      for (const loc of locs) {
        requireConnected(loc.source);
        await latency(loc.source);
        const { dir: parent, name } = parentOf(loc);
        if (!parent.children.delete(name)) fail("not_found", loc.path);
      }
    },
    async readText(loc, maxBytes) {
      requireConnected(loc.source);
      await latency(loc.source);
      const node = lookup(loc);
      if (node.kind !== "file") fail("is_a_directory", loc.path);
      const lines = Array.from({ length: 80 }, (_, i) => `${String(i + 1).padStart(3)}  mock line for ${split(loc.path).at(-1)} — ${"lorem ipsum ".repeat(1 + (i % 6))}`);
      const text = `# ${loc.path}\n\nMock preview — ${node.size} bytes.\n\n${lines.join("\n")}\n`;
      return { text: text.slice(0, maxBytes), truncated: node.size > maxBytes };
    },
    async openExternal(loc) {
      console.info("[mock] open externally:", loc);
      if (loc.source.kind === "local") return null;
      requireConnected(loc.source);
      const node = lookup(loc);
      if (node.kind !== "file") fail("is_a_directory", loc.path);
      const id = crypto.randomUUID();
      const s: EditSession = {
        id, location: loc, localPath: `/tmp/vps-files/edit/${id.slice(0, 8)}/${split(loc.path).at(-1)}`,
        state: "downloading", error: null, lastSync: null,
      };
      edits.set(id, s);
      const conflict = loc.source.host === CONFLICT_HOST;
      editPlan(id, [
        [Math.min(4000, 500 + node.size / 40_000), () => editSet(id, "open")],
        // The user saves in their editor a few seconds later.
        [3500, () => editSet(id, "uploading")],
        [900, () => {
          if (conflict) editSet(id, "conflict");
          else {
            touchRemote(loc);
            editSet(id, "synced", { lastSync: Date.now() });
          }
        }],
      ]);
      return s;
    },
    async editList() {
      return [...edits.values()];
    },
    async editStop(id) {
      if (!edits.has(id)) fail("not_found", id);
      editPlan(id, []);
      editTimers.delete(id);
      editSet(id, "closed");
    },
    async editResolve(id, action) {
      const s = edits.get(id);
      if (!s) fail("not_found", id);
      if (s.state !== "conflict") fail("other", "No conflict to resolve");
      editPlan(id, action === "upload"
        ? [[0, () => editSet(id, "uploading")], [900, () => { touchRemote(s.location); editSet(id, "synced", { lastSync: Date.now() }); }]]
        : [[0, () => editSet(id, "downloading")], [700, () => editSet(id, "open")]]);
    },

    async transferStart(req) {
      const destDir = lookup(req.destDir);
      if (destDir.kind !== "dir") fail("not_a_directory", req.destDir.path);
      let bytes = 0, files = 0;
      for (const s of req.sources) {
        requireConnected(s.source);
        const t = totalSize(lookup(s));
        bytes += t.bytes; files += t.files;
      }
      const id = crypto.randomUUID();
      const job: JobSnapshot = {
        id, op: req.op, state: "running", sources: req.sources, destDir: req.destDir,
        totalBytes: bytes, doneBytes: 0, totalFiles: files, doneFiles: 0,
        current: null, bytesPerSec: 0, error: null, createdAt: Date.now(), finishedAt: null,
      };
      jobs.set(id, job);
      emit("transfer:update", job);
      runJob(id, () => {
        // Materialize the result in the destination tree.
        for (const s of req.sources) {
          const name = split(s.path).at(-1)!;
          let target = name;
          for (let n = 2; destDir.children.has(target); n++) {
            if (req.conflict === "skip") return;
            if (req.conflict === "overwrite" || req.conflict === "merge") break;
            const dot = name.lastIndexOf(".");
            target = dot > 0 ? `${name.slice(0, dot)} ${n}${name.slice(dot)}` : `${name} ${n}`;
          }
          destDir.children.set(target, clone(lookup(s)));
          if (req.op === "move") parentOf(s).dir.children.delete(name);
        }
      });
      return id;
    },
    async transferList() {
      return [...jobs.values()];
    },
    async transferPause(id) {
      setJobState(id, "paused");
    },
    async transferResume(id) {
      setJobState(id, "running");
    },
    async transferCancel(id) {
      clearInterval(jobTimers.get(id));
      jobTimers.delete(id);
      setJobState(id, "cancelled");
    },
    async transferClearFinished() {
      for (const [id, j] of jobs) {
        if (j.state === "completed" || j.state === "failed" || j.state === "cancelled") jobs.delete(id);
      }
    },

    async diskFree(loc) {
      requireConnected(loc.source);
      await latency(loc.source);
      const tb = 1024 ** 4;
      return loc.source.kind === "local"
        ? { free: 0.12 * tb, total: 0.5 * tb }
        : { free: 0.031 * tb, total: 0.08 * tb };
    },

    async mediaUrl(loc) {
      // Public sample media so the player UI can be exercised in a browser.
      const ext = loc.path.split(".").pop()?.toLowerCase();
      if (ext === "mp3") return "https://interactive-examples.mdn.mozilla.net/media/cc0-audio/t-rex-roar.mp3";
      if (ext === "jpg" || ext === "png" || ext === "webp") return `https://picsum.photos/seed/${encodeURIComponent(loc.path.replace(/[^\w.-]+/g, "-"))}/1600/1000`;
      return "https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm";
    },

    async mediaBase() {
      return ""; // no media server in the browser: media.ts returns null URLs
    },
    async externalPlayers() {
      return ["mpv"];
    },
    async openInPlayer(loc) {
      console.info("[mock] open in player:", loc);
    },

    async promptRespond(id, accept, secret, remember) {
      const resolve = pendingPrompts.get(id);
      if (!resolve) fail("not_found", `prompt ${id}`);
      pendingPrompts.delete(id);
      resolve({ accept, secret, remember: remember ?? false });
    },

    async on(event, handler) {
      const set = listeners.get(event) ?? new Set();
      set.add(handler as Handler);
      listeners.set(event, set);
      return () => set.delete(handler as Handler);
    },
  };
}
