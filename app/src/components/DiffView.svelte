<!-- One file's changes since the last commit, as a plain unified diff. -->
<script lang="ts">
  import type { ChangeKind, FileDiff } from "../lib/types";

  let { diff }: { diff: FileDiff } = $props();

  const VERB: Record<ChangeKind, string> = { added: "added", modified: "changed", deleted: "deleted", renamed: "renamed" };
  const SIGN = { context: " ", added: "+", removed: "−" } as const;

  const image = $derived(/\.(jpe?g|png|gif|webp|heic|avif)$/i.test(diff.path));
  const note = $derived.by(() => {
    if (diff.binary) return `${image ? "Image" : "File"} ${VERB[diff.kind]}.`;
    if (diff.hunks.length) return null;
    if (diff.kind === "renamed") return "Renamed; the text is the same.";
    if (diff.kind === "added") return "A new, empty file.";
    if (diff.kind === "deleted") return "An empty file, deleted.";
    return "Only invisible changes, such as file permissions.";
  });
  const label = $derived(`Changes to ${diff.path}`);
</script>

{#if note}
  <p class="note">{note}</p>
{:else}
  <div class="diff" role="group" aria-label={label}>
    {#each diff.hunks as hunk, i (i)}
      {#if i > 0 || hunk.oldStart > 1 || hunk.newStart > 1}<div class="gap" aria-hidden="true">…</div>{/if}
      {#each hunk.lines as line, j (j)}
        <div class="line {line.kind}"><span class="sign" aria-hidden="true">{SIGN[line.kind]}</span><span
            class="text">{line.text || " "}</span
          >{#if line.kind !== "context"}<span class="visually-hidden">{line.kind}</span>{/if}</div>
      {/each}
    {/each}
  </div>
{/if}

<style>
  .note {
    margin: 2px 0 6px 24px;
    color: var(--color-muted);
    font-style: italic;
  }

  .diff {
    max-height: 40vh;
    margin: 2px 0 8px;
    padding: 4px 0;
    overflow: auto;
    border: 1px solid var(--color-line);
    border-radius: var(--radius);
    font-family: var(--font-source);
    font-size: 12px;
    line-height: 1.5;
  }

  .line {
    display: flex;
    padding: 0 8px 0 0;
  }

  .sign {
    flex: none;
    width: 2.2ch;
    text-align: center;
    color: var(--color-muted);
    user-select: none;
    -webkit-user-select: none;
  }

  .text {
    flex: 1;
    min-width: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .added {
    background: color-mix(in srgb, var(--color-accent) 12%, transparent);
  }

  .added .sign {
    color: var(--color-accent);
  }

  .removed {
    background: color-mix(in srgb, var(--color-danger) 11%, transparent);
  }

  .removed .sign {
    color: var(--color-danger);
  }

  .context .text {
    color: var(--color-muted);
  }

  .gap {
    padding: 0 0 0 2.2ch;
    color: var(--color-muted);
  }
</style>
