<!-- The Sync review (PLAN §5): files changed both here and on GitHub, each with a choice; what
     will apply on its own; and Apply, Cancel or Resolve with git. Nothing is on disk until Apply. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { pathLabel } from "../lib/journal";
  import {
    choiceLabel,
    defaultPick,
    mergeCounts,
    overlapNote,
    type Review,
  } from "../lib/review.svelte";
  import { changeWord, commitMessageValid, plural, relativeTime, subject } from "../lib/sync";
  import type { Choice, CommitInfo, Overlap } from "../lib/types";
  import Icon from "./Icon.svelte";
  import Sheet from "./Sheet.svelte";

  let { app, review }: { app: AppState; review: Review } = $props();

  const preview = $derived(review.preview);
  const changes = $derived(app.sync.changes);
  const remaining = $derived(preview.overlaps.filter((o) => !review.picks[o.path]).length);
  const gitReady = $derived(!review.busy && (changes.length === 0 || commitMessageValid(review.gitMessage)));

  const lead = $derived(
    preview.overlaps.length
      ? "Some files changed both here and on GitHub. Nothing has changed on disk yet: choose what to keep of each, then Apply."
      : "Nothing has changed on disk yet.",
  );

  const planText = $derived.by(() => {
    const later = "files you resolve stay as changes for your next commit";
    switch (preview.plan) {
      case "fast_forward":
        return preview.overlaps.length ? `Apply brings in GitHub's commits; ${later}.` : "Apply brings in GitHub's commits.";
      case "rebase":
        return `Apply places your ${plural(preview.localCount, "unpushed commit")} after GitHub's${preview.overlaps.length ? `; ${later}` : ""}.`;
      case "merge":
        return `Your ${plural(preview.localCount, "unpushed commit")} and GitHub's can't be put in a line, so Apply joins them with a merge commit:`;
    }
  });

  function choose(overlap: Overlap, choice: Choice) {
    if (review.busy) return;
    if (choice === "merged") app.openMerge(overlap.path);
    else review.setPick(overlap.path, { choice });
  }

  /** What the file's current choice will do, in words. */
  function outcome(overlap: Overlap): string {
    const pick = review.picks[overlap.path];
    const counts = overlap.suggestion && mergeCounts(overlap.suggestion.chunks);
    if (!pick) {
      if (counts?.clashes) {
        return `Both changed the same lines in ${counts.clashes === 1 ? "one place" : `${counts.clashes} places`}: choose a version, or Merge… to choose line by line.`;
      }
      return "Choose which version to keep.";
    }
    switch (pick.choice) {
      case "mine":
        return overlap.kind === "deleted_modified" ? "It stays deleted." : "Your version stays as it is.";
      case "theirs":
        if (overlap.kind === "modified_deleted") return "It is deleted, as on GitHub, with your changes to it.";
        return overlap.kind === "deleted_modified" ? "GitHub's version comes back." : "GitHub's version replaces yours.";
      case "both":
        return `Yours stays; GitHub's is saved next to it as ${overlap.copy}.`;
      case "merged": {
        const prefilled = defaultPick(overlap);
        if (!counts || prefilled?.choice !== "merged" || prefilled.text !== pick.text) return "Merged in the merge view.";
        const parts = [counts.mine && `${plural(counts.mine, "change")} of yours`, counts.theirs && `${plural(counts.theirs, "change")} from GitHub`];
        return `Merged: ${parts.filter(Boolean).join(" and ") || "the same changes on both sides"}, no clashes.`;
      }
    }
  }

  function close() {
    if (review.busy) return;
    if (review.resolveWithGit) review.resolveWithGit = false;
    else app.cancelReview();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key !== "Enter" || !(e.metaKey || e.ctrlKey) || e.isComposing) return;
    e.preventDefault();
    if (review.resolveWithGit) void app.resolveWithGit();
    else void app.applyReview();
  }

  const firstOpen = $derived(preview.overlaps.find((o) => !review.picks[o.path])?.path);
</script>

{#snippet commitList(commits: CommitInfo[], count: number)}
  <ul class="commits">
    {#each commits as commit (commit.id)}
      <li>
        <span class="subject">{subject(commit)}</span>
        <span class="by">{commit.author} · {relativeTime(commit.time)}</span>
      </li>
    {/each}
    {#if count > commits.length}<li class="by">and {count - commits.length} more</li>{/if}
  </ul>
{/snippet}

{#snippet fileName(path: string)}
  {@const label = pathLabel(path)}
  <span class="title" class:mono={!label.entry || !label.when}>{label.title}</span>
  {#if label.when}<span class="when">{label.when}</span>{/if}
{/snippet}

<Sheet label="Sync review" wide onclose={close}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="review" {onkeydown}>
    <div class="head">
      <h2>{review.resolveWithGit ? "Resolve with git" : "Pull from GitHub"}</h2>
      <button type="button" class="close" aria-label="Close" title="Cancel the review (Esc)" disabled={review.busy} onclick={() => app.cancelReview()}>
        <Icon name="close" />
      </button>
    </div>

    {#if review.notice}<p class="notice" role="status">{review.notice}</p>{/if}

    {#if !review.resolveWithGit}
      <p class="lead">{lead}</p>

      <details class="group">
        <summary>{plural(preview.incomingCount, "commit")} from GitHub</summary>
        {@render commitList(preview.incoming, preview.incomingCount)}
      </details>
      {#if preview.localCount}
        <details class="group">
          <summary>{plural(preview.localCount, "unpushed commit")} here</summary>
          {@render commitList(preview.local, preview.localCount)}
        </details>
      {/if}

      {#if preview.overlaps.length}
        <section>
          <h3>Changed on both sides</h3>
          <ul class="files">
            {#each preview.overlaps as overlap (overlap.path)}
              {@const pick = review.picks[overlap.path]}
              {@const label = pathLabel(overlap.path)}
              <li class="file" class:open={!pick}>
                <div class="name">{@render fileName(overlap.path)}</div>
                <p class="note">{overlapNote(overlap)}</p>
                <div class="choices" role="group" aria-label={`What to keep of ${label.title}`}>
                  {#each overlap.choices as choice (choice)}
                    <button
                      type="button"
                      aria-pressed={pick?.choice === choice}
                      disabled={review.busy}
                      data-autofocus={!app.mobile && !review.ready && overlap.path === firstOpen && choice === overlap.choices[0]
                        ? true
                        : undefined}
                      onclick={() => choose(overlap, choice)}
                    >
                      {choiceLabel(overlap, choice)}
                    </button>
                  {/each}
                </div>
                <p class="outcome" class:waiting={!pick}>
                  {#if pick?.choice === "both"}
                    Yours stays; GitHub's is saved next to it as <code>{overlap.copy}</code>.
                  {:else}
                    {outcome(overlap)}
                  {/if}
                  {#if pick?.choice === "merged"}
                    <button type="button" class="link" disabled={review.busy} onclick={() => app.openMerge(overlap.path)}>
                      Check the merge
                    </button>
                  {/if}
                </p>
              </li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if preview.auto.length}
        <details class="group">
          <summary>{plural(preview.auto.length, "file")} changed on GitHub only: {preview.auto.length === 1 ? "it applies" : "they apply"} automatically</summary>
          <ul class="auto">
            {#each preview.auto as change (change.path)}
              <li>
                <span class="kind">{changeWord(change.kind)}</span>
                {#if change.oldPath}<span class="mono">{change.oldPath} →</span>{/if}
                {@render fileName(change.path)}
              </li>
            {/each}
          </ul>
        </details>
      {/if}

      <p class="plan">{planText}</p>
      {#if preview.plan === "merge"}
        <textarea
          aria-label="Merge commit message"
          rows={Math.min(8, review.message.split("\n").length + 1)}
          value={review.message}
          readonly={review.busy}
          oninput={(e) => review.editMessage(e.currentTarget.value)}
        ></textarea>
      {/if}

      {#if review.error}<p class="error" role="alert">{review.error}</p>{/if}

      <div class="actions">
        {#if preview.overlaps.length}
          <button type="button" class="link git" disabled={review.busy} onclick={() => (review.resolveWithGit = true)}>
            Resolve with git…
          </button>
        {/if}
        <span class="grow"></span>
        {#if remaining}<span class="hint">{remaining === 1 ? "Choose for 1 more file" : `Choose for ${remaining} more files`}</span>{/if}
        <button type="button" class="button cancel" disabled={review.busy} onclick={() => app.cancelReview()}>Cancel</button>
        <button
          type="button"
          class="button primary apply"
          disabled={!review.ready || review.busy}
          data-autofocus={review.ready ? true : undefined}
          onclick={() => app.applyReview()}
        >
          {#if review.busy}<span class="spinner" aria-hidden="true"></span>{/if}
          Apply
        </button>
      </div>
    {:else}
      <p>
        This starts a standard git merge, as <code>git merge origin/{preview.branch}</code> does, and leaves it for you to
        finish with your editor, <code>git mergetool</code> or the command line. In these files, git writes conflict markers
        (<code>&lt;&lt;&lt;&lt;&lt;&lt;&lt;</code>) where the changes clash:
      </p>
      <ul class="paths">
        {#each preview.overlaps as overlap (overlap.path)}<li class="mono">{overlap.path}</li>{/each}
      </ul>
      {#if changes.length}
        <p>
          First, your uncommitted changes to {plural(changes.length, "file")} are committed, so git can merge them. Abort
          merge makes them uncommitted again.
        </p>
        <textarea
          data-autofocus
          {@attach (node) => node.focus()}
          bind:value={review.gitMessage}
          rows="2"
          aria-label="Commit message for your changes"
          placeholder="What changed?"
          readonly={review.busy}
        ></textarea>
      {/if}
      <p class="muted">Until the merge is committed, Pull and Commit & Push are off.</p>
      {#if review.error}<p class="error" role="alert">{review.error}</p>{/if}
      <div class="actions">
        <button type="button" class="button" disabled={review.busy} onclick={() => (review.resolveWithGit = false)}>Back</button>
        <span class="grow"></span>
        <button
          type="button"
          class="button primary"
          data-autofocus={changes.length ? undefined : true}
          {@attach (node) => {
            if (!changes.length) node.focus();
          }}
          disabled={!gitReady}
          onclick={() => app.resolveWithGit()}
        >
          {#if review.busy}<span class="spinner" aria-hidden="true"></span>{/if}
          Start the merge
        </button>
      </div>
    {/if}
  </div>
</Sheet>

<style>
  .review {
    display: flex;
    flex-direction: column;
    gap: 10px;
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
    margin: 4px 0 6px;
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

  .notice {
    padding: 6px 10px;
    border-left: 2px solid var(--color-accent);
    background: var(--color-accent-soft);
  }

  .lead {
    color: var(--color-muted);
  }

  .group summary {
    color: var(--color-muted);
    cursor: default;
  }

  .group summary:hover {
    color: var(--color-text);
  }

  .commits,
  .auto,
  .paths {
    margin: 6px 0 2px;
    padding: 0 0 0 16px;
    list-style: none;
  }

  .commits li {
    display: flex;
    flex-wrap: wrap;
    column-gap: 8px;
    padding: 2px 0;
  }

  .subject {
    overflow-wrap: anywhere;
  }

  .by {
    color: var(--color-muted);
    font-size: 12px;
  }

  .files {
    margin: 0;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--color-line);
  }

  .file {
    display: grid;
    gap: 4px;
    padding: 10px 0;
    border-bottom: 1px solid var(--color-line);
  }

  .name {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    column-gap: 10px;
  }

  .title {
    font-family: var(--font-styled);
    font-size: 16px;
    overflow-wrap: anywhere;
  }

  .title.mono,
  .mono {
    font-family: var(--font-source);
    font-size: 12px;
  }

  .when {
    color: var(--color-muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .note {
    color: var(--color-muted);
    font-size: 12px;
  }

  .choices {
    display: flex;
    flex-wrap: wrap;
    justify-self: start;
    gap: 2px;
    padding: 1px;
    border: 1px solid var(--color-line);
    border-radius: var(--radius);
  }

  .choices button {
    padding: 3px 10px;
    border-radius: 4px;
    color: var(--color-muted);
  }

  .choices button[aria-pressed="true"] {
    background: var(--color-selected);
    color: var(--color-text);
  }

  .file.open .choices {
    border-color: color-mix(in srgb, var(--color-accent) 55%, transparent);
  }

  .outcome {
    font-size: 12px;
  }

  .outcome.waiting {
    color: var(--color-accent);
  }

  .auto li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    column-gap: 8px;
    padding: 2px 0;
  }

  .auto .title {
    font-size: 14px;
  }

  .kind {
    min-width: 4.5em;
    color: var(--color-muted);
    font-size: 12px;
  }

  .plan {
    color: var(--color-muted);
  }

  textarea {
    display: block;
    width: 100%;
    resize: vertical;
    font-family: var(--font-ui);
    line-height: 1.45;
  }

  code {
    font-family: var(--font-source);
    font-size: 12px;
  }

  .error {
    color: var(--color-danger);
  }

  .actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 4px;
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

  .hint {
    color: var(--color-muted);
    font-size: 12px;
  }

  .link {
    padding: 0;
    color: var(--color-accent);
    font-size: inherit;
  }

  .link:hover:not(:disabled) {
    background: none;
    text-decoration: underline;
  }

  .outcome .link {
    margin-left: 4px;
  }

  .git {
    color: var(--color-muted);
  }

  :global([data-layout="phone"]) .choices {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    justify-self: stretch;
  }

  :global([data-layout="phone"]) .choices button,
  :global([data-layout="phone"]) .actions .button,
  :global([data-layout="phone"]) .link {
    min-height: var(--touch);
  }

  /* The phone: what is left to choose, then Cancel and Apply side by side, then the escape hatch. */
  :global([data-layout="phone"]) .actions .grow {
    display: none;
  }

  :global([data-layout="phone"]) .actions .hint {
    flex-basis: 100%;
    text-align: center;
  }

  :global([data-layout="phone"]) .actions .cancel,
  :global([data-layout="phone"]) .actions .apply {
    flex: 1;
    justify-content: center;
  }

  :global([data-layout="phone"]) .actions .git {
    order: 1;
    flex-basis: 100%;
  }

  :global([data-layout="phone"]) .outcome .link {
    display: block;
    margin: 0;
    text-align: left;
  }

  :global([data-layout="phone"]) .group summary {
    padding: 12px 0;
  }

  :global([data-layout="phone"]) .actions .button {
    padding: 0 16px;
  }

  :global([data-layout="phone"]) .close {
    width: var(--touch);
    height: var(--touch);
    margin-right: -8px;
  }
</style>
