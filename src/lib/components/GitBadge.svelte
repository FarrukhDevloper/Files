<!-- Git status of one entry: a colored letter (M, A, D, R, U, !). -->
<script lang="ts">
  import type { GitFileStatus } from "$lib/ipc/types";
  import { gitLabel, gitLetter } from "$lib/util/git";

  let { status, round = false }: { status: GitFileStatus; round?: boolean } = $props();
</script>

<span class="git {status}" class:round title={gitLabel[status]} aria-label="Git: {gitLabel[status]}">{gitLetter[status]}</span>

<style>
  .git {
    flex: none;
    font: 700 11px/1 var(--font-mono);
    color: var(--git-c);
  }
  .round {
    display: grid;
    place-items: center;
    width: 17px;
    height: 17px;
    border-radius: 50%;
    background: var(--git-c);
    color: #fff;
    font-size: 10px;
    box-shadow: 0 0 0 2px var(--bg-window);
  }
  .modified { --git-c: var(--git-modified); }
  .added { --git-c: var(--git-added); }
  .untracked { --git-c: var(--git-untracked); }
  .renamed { --git-c: var(--git-renamed); }
  .deleted, .conflicted { --git-c: var(--git-deleted); }
</style>
