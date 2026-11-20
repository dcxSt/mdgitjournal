<!-- What the last pull did (the summary notice's Details): GitHub's commits, the files it changed,
     the choices made in the review, and Undo while it is possible. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { pathLabel } from "../lib/journal";
  import { resolutionText } from "../lib/review.svelte";
  import { changeWord, plural, rebasedText, relativeTime, subject } from "../lib/sync";
  import type { SyncSummary } from "../lib/types";
  import Icon from "./Icon.svelte";
  import Sheet from "./Sheet.svelte";

  let { app, summary }: { app: AppState; summary: SyncSummary } = $props();

  let undoing = $state(false);
  let error = $state<string | null>(null);

  const title = $derived(summary.plan === "merge" ? "Merged from GitHub" : "Pulled from GitHub");
  const exists = (path: string) => app.entries.some((e) => e.file === path);

  function close() {
    if (!undoing) app.closeOverlay();
  }

  function openEntry(file: string) {
    app.closeOverlay();
    void app.openEntry(file);
  }

  async function undo() {
    undoing = true;
    error = null;
    const failure = await app.undoSync();
    undoing = false;
    if (failure) error = failure.message;
  }
</script>

{#snippet fileName(path: string)}
  {@const label = pathLabel(path)}
  {#if label.entry && exists(path)}
    <button type="button" class="link title" class:mono={!label.when} onclick={() => openEntry(path)}>{label.title}</button>
  {:else}
    <span class="title" class:mono={!label.entry || !label.when}>{label.title}</span>
  {/if}
  {#if label.when}<span class="when">{label.when}</span>{/if}
{/snippet}

<Sheet label="Pull details" wide onclose={close}>
  <div class="summary">
    <div class="head">
      <h2>{title}</h2>
      <button type="button" class="close" aria-label="Close" disabled={undoing} onclick={close}>
        <Icon name="close" />
      </button>
    </div>
    <p class="muted">{relativeTime(summary.at)}</p>

    <section>
      <h3>{plural(summary.incomingCount, "commit")} from GitHub</h3>
      <ul class="commits">
        {#each summary.incoming as commit (commit.id)}
          <li>
            <span>{subject(commit)}</span>
            <span class="by">{commit.author} · {relativeTime(commit.time)}</span>
          </li>
        {/each}
        {#if summary.incomingCount > summary.incoming.length}
          <li class="by">and {summary.incomingCount - summary.incoming.length} more</li>
        {/if}
      </ul>
      {#if summary.rebased > 0}<p>{rebasedText(summary)}</p>{/if}
      {#if summary.mergeCommit}
        <p>
          A merge commit, <code title={summary.mergeCommit}>{summary.mergeCommit.slice(0, 7)}</code>, joins your unpushed
          commits and GitHub's.
        </p>
      {/if}
    </section>

    {#if summary.files.length}
      <section>
        <h3>{plural(summary.files.length, "file")} changed</h3>
        <ul class="files">
          {#each summary.files as change (change.path)}
            <li>
              <span class="kind">{changeWord(change.kind)}</span>
              {#if change.oldPath}<span class="mono">{change.oldPath} →</span>{/if}
              {@render fileName(change.path)}
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if summary.resolved.length}
      <section>
        <h3>Your choices</h3>
        <ul class="files">
          {#each summary.resolved as resolved (resolved.path)}
            <li>
              {@render fileName(resolved.path)}
              <span class="muted">{resolutionText(resolved, resolved.choice)}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      {#if app.canUndo}
        <button type="button" class="button" disabled={undoing} onclick={undo}>
          {#if undoing}<span class="spinner" aria-hidden="true"></span>{/if}
          Undo this pull
        </button>
        <span class="muted small">Puts back exactly what was here before it.</span>
      {/if}
      <span class="grow"></span>
      <button type="button" class="button primary" data-autofocus disabled={undoing} onclick={close}>Close</button>
    </div>
  </div>
</Sheet>

<style>
  .summary {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 500;
  }

  h3 {
    margin: 6px 0 4px;
    font-size: 11px;
    font-weight: 500;
    color: var(--color-muted);
  }

  p {
    margin: 0;
  }

  .close {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    color: var(--color-muted);
  }

  ul {
    margin: 0 0 4px;
    padding: 0;
    list-style: none;
  }

  li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    column-gap: 8px;
    padding: 2px 0;
  }

  .by,
  .when,
  .small {
    color: var(--color-muted);
    font-size: 12px;
  }

  .when {
    font-variant-numeric: tabular-nums;
  }

  .kind {
    min-width: 4.5em;
    color: var(--color-muted);
    font-size: 12px;
  }

  .title {
    font-family: var(--font-styled);
    font-size: 15px;
    overflow-wrap: anywhere;
  }

  .mono,
  .title.mono,
  code {
    font-family: var(--font-source);
    font-size: 12px;
  }

  .link {
    padding: 0;
    color: var(--color-accent);
    text-align: left;
  }

  .link:hover {
    background: none;
    text-decoration: underline;
  }

  .error {
    color: var(--color-danger);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
  }

  .actions .button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 28px;
  }

  .grow {
    flex: 1;
  }

  :global([data-layout="phone"]) .actions .button {
    min-height: var(--touch);
    padding: 0 14px;
  }

  :global([data-layout="phone"]) .close {
    width: var(--touch);
    height: var(--touch);
    margin-right: -8px;
  }
</style>
