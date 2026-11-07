// Display formatting: sizes (decimal units, like Finder), dates, rates.

const nf0 = new Intl.NumberFormat("en-GB", { maximumFractionDigits: 0 });
const nf1 = new Intl.NumberFormat("en-GB", { maximumFractionDigits: 1 });
const nf2 = new Intl.NumberFormat("en-GB", { maximumFractionDigits: 2 });

export function formatSize(bytes: number): string {
  if (bytes < 1000) return bytes === 1 ? "1 byte" : `${nf0.format(bytes)} bytes`;
  if (bytes < 1e6) return `${nf0.format(bytes / 1e3)} KB`;
  if (bytes < 1e9) return `${nf1.format(bytes / 1e6)} MB`;
  if (bytes < 1e12) return `${nf2.format(bytes / 1e9)} GB`;
  return `${nf2.format(bytes / 1e12)} TB`;
}

export function formatRate(bytesPerSec: number): string {
  return `${formatSize(Math.round(bytesPerSec))}/s`;
}

const timeFmt = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
const dateFmt = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", year: "numeric" });
const fullFmt = new Intl.DateTimeFormat("en-GB", { dateStyle: "long", timeStyle: "medium" });

function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** "Today, 14:02" · "Yesterday, 09:10" · "9 Oct 2026". */
export function formatDate(ms: number | null): string {
  if (ms === null) return "—";
  const today = startOfDay(Date.now());
  if (ms >= today && ms < today + 86_400_000) return `Today, ${timeFmt.format(ms)}`;
  if (ms >= today - 86_400_000 && ms < today) return `Yesterday, ${timeFmt.format(ms)}`;
  return dateFmt.format(ms);
}

export function formatDateLong(ms: number | null): string {
  return ms === null ? "—" : fullFmt.format(ms);
}

/** "4 s left" · "3 min left" · "1 h 5 min left". */
export function formatEta(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "";
  const s = Math.ceil(seconds);
  if (s < 60) return `${s} s left`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min left`;
  return `${Math.floor(m / 60)} h ${m % 60} min left`;
}

/** 0o755 + dir → "drwxr-xr-x". */
export function formatMode(mode: number | null, isDir: boolean): string {
  if (mode === null) return "—";
  const rwx = (n: number) => `${n & 4 ? "r" : "-"}${n & 2 ? "w" : "-"}${n & 1 ? "x" : "-"}`;
  const octal = (mode & 0o777).toString(8).padStart(3, "0");
  return `${isDir ? "d" : "-"}${rwx(mode >> 6)}${rwx(mode >> 3)}${rwx(mode)} (${octal})`;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${nf0.format(n)} ${n === 1 ? one : many}`;
}
