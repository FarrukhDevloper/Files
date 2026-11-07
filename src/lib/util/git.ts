// How git states read in the UI (letters like VS Code, colors in tokens.css).

import type { GitFileStatus, GitInfo } from "$lib/ipc/types";

export const gitLetter: Record<GitFileStatus, string> = {
  modified: "M", added: "A", deleted: "D", renamed: "R", untracked: "U", conflicted: "!",
};

export const gitLabel: Record<GitFileStatus, string> = {
  modified: "Modified", added: "Added", deleted: "Deleted", renamed: "Renamed", untracked: "Untracked", conflicted: "Conflict",
};

/** "Open on GitHub" etc., from the web URL's host. */
export function hostingName(webUrl: string): string {
  let host = "";
  try { host = new URL(webUrl).hostname; } catch { /* not a URL */ }
  if (host.endsWith("github.com")) return "GitHub";
  if (host.includes("gitlab")) return "GitLab";
  if (host.endsWith("bitbucket.org")) return "Bitbucket";
  if (host.includes("codeberg")) return "Codeberg";
  return "the Web";
}

/** Chip text: "main", or "HEAD 3f9c2ab" when detached. */
export function branchLabel(g: GitInfo): string {
  return g.branch ?? (g.head ? `HEAD ${g.head}` : "no commits");
}
