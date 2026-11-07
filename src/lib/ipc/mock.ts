// In-memory stand-in for the Rust backend, used when the UI runs in a plain
// browser (`pnpm dev`). It honours the same contract as api.ts so screens can
// be built and demoed before the backend exists. Keep it boring and faithful:
// errors use real codes, remote calls have latency, transfers emit progress.

import type { Api } from "./api";
import type {
  ConnStatus, Entry, EventMap, HostInfo, JobSnapshot, Location, Place,
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
          Projects: dir({ "vps-files": dir({ "README.md": file(2_048) }) }),
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

const hosts: Omit<HostInfo, "status">[] = [
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
const trustedHosts = new Set(["demo-web", "demo-pass", "demo-down"]);
const pendingPrompts = new Map<string, (answer: { accept: boolean; secret: string | null }) => void>();

function ask(request: PromptRequest): Promise<{ accept: boolean; secret: string | null }> {
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

export function createMockApi(): Api {
  return {
    async listHosts() {
      return hosts.map((h) => ({ ...h, status: status.get(h.alias)! }));
    },
    async connectHost(alias) {
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
      if (!trustedHosts.has(alias)) {
        const answer = await ask({
          kind: "hostKey", id: crypto.randomUUID(), host: alias, algorithm: "ED25519",
          fingerprint: "SHA256:q3Vn1dHkPZ0rM9yQe5o8wXkz7cB2fTgLjA4sU6vN1hE",
        });
        if (!answer.accept) failWith({ code: "cancelled", message: "Host key not trusted" });
        trustedHosts.add(alias);
      }
      if (alias === "demo-pass") {
        const answer = await ask({ kind: "password", id: crypto.randomUUID(), host: alias, user: "root" });
        if (!answer.accept) failWith({ code: "cancelled", message: "Password prompt cancelled" });
        if (answer.secret !== "demo") failWith({ code: "auth", message: "Permission denied (publickey,password)" });
      }
      status.set(alias, { state: "connected" });
      emit("conn:status", { host: alias, status: { state: "connected" } });
    },
    async disconnectHost(alias) {
      status.set(alias, { state: "disconnected" });
      emit("conn:status", { host: alias, status: { state: "disconnected" } });
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

    async promptRespond(id, accept, secret) {
      const resolve = pendingPrompts.get(id);
      if (!resolve) fail("not_found", `prompt ${id}`);
      pendingPrompts.delete(id);
      resolve({ accept, secret });
    },

    async on(event, handler) {
      const set = listeners.get(event) ?? new Set();
      set.add(handler as Handler);
      listeners.set(event, set);
      return () => set.delete(handler as Handler);
    },
  };
}
