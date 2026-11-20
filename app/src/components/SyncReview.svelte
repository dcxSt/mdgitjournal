<!-- The Sync review (PLAN §5): files changed both here and on GitHub, each with a choice; what
     will apply on its own; and Apply, Cancel or Resolve with git. Nothing is on disk until Apply. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { pathLabel } from "../lib/journal";
  import {
    choiceLabel,
    defaultPick,
    mergeCounts,
    overlapName,
    overlapNote,
    type Review,
  } from "../lib/review.svelte";
  import { changeWord, commitMessageValid, plural, relativeTime, subject } from "../lib/sync";
  import type { Choice, CommitInfo, Overlap } from "../lib/types";
  import Icon from "./Icon.svelte";
  import Sheet from "./Sheet.svelte";

  let { app, review }: { app: AppState; review: Review } = $props();

  const preview = $derived(review.preview);
  const remaining = $derived(preview.overlaps.filter((o) => !review.picks[o.path]).length);
  /** Resolve with git commits these files' uncommitted changes first, so git can merge them. */
  const precommit = $derived(preview.gitMerge.uncommitted);
  const gitReady = $derived(!review.busy && (precommit.length === 0 || commitMessageValid(review.gitMessage)));
  /** There is no git on the phone to finish a merge with; the merge view covers it there. */
  const gitOffered = $derived(preview.overlaps.length > 0 && app.info?.platform !== "ios");
  const conflicted = $derived(new Set(preview.gitMerge.conflicted));
  const withMarkers = $derived(preview.overlaps.filter((o) => sides(o).some((p) => conflicted.has(p))));
  const mergedByGit = $derived(preview.overlaps.filter((o) => !withMarkers.includes(o)));
  /** In a merge, files whose resolution the merge commit doesn't record: they stay changes to commit. */
  const uncommittedOnly = $derived(preview.plan === "merge" && preview.overlaps.some((o) => !o.committed));
  /** Esc or a click outside asked to close a review with work in it. */
  let discarding = $state(false);

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
      case "merge": {
        const commits = plural(preview.localCount, "unpushed commit");
        if (!preview.overlaps.length) return `Your ${commits} and GitHub's can't be put in a line, so Apply joins them with a merge commit:`;
        const joins = `Apply joins your ${commits} and GitHub's with a merge commit`;
        const recorded = preview.overlaps.some((o) => o.committed);
        if (recorded && uncommittedOnly) return `${joins}, which records how you resolved the files your commits changed; the rest stays as changes for your next commit:`;
        if (recorded) return `${joins}, which records how you resolved them:`;
        return `${joins}; ${later}:`;
      }
    }
  });

  function sides(overlap: Overlap): string[] {
    return [overlap.path, overlap.minePath, overlap.theirsPath].filter((p): p is string => p !== null);
  }

  function choose(overlap: Overlap, choice: Choice) {
    if (review.busy) return;
    if (choice === "merged") app.openMerge(overlap.path);
    else review.setPick(overlap.path, { choice });
  }

  /** “Long run” for an entry, the path for any other file. */
  function title(path: string | null): string {
    return path === null ? "" : `“${pathLabel(path).title}”`;
  }

  /** What the file's current choice will do, in words (Keep both has its own line, with the name). */
  function outcome(overlap: Overlap): string {
    const pick = review.picks[overlap.path];
    const counts = overlap.suggestion && mergeCounts(overlap.suggestion.chunks);
    if (!pick) {
      if (counts?.clashes) {
        return `Both changed the same lines in ${counts.clashes === 1 ? "one place" : `${counts.clashes} places`}: choose a version, or Merge… to choose line by line.`;
      }
      return "Choose which version to keep.";
    }
    const later = preview.plan === "merge" && !overlap.committed && pick.choice !== "theirs" ? " It stays as a change for your next commit." : "";
    const renamed = overlap.kind === "modified_renamed" || overlap.kind === "renamed_modified" || overlap.kind === "both_renamed";
    switch (pick.choice) {
      case "mine":
        if (overlap.kind === "deleted_modified" || overlap.kind === "deleted_renamed") return "It stays deleted.";
        if (renamed) return `Your version stays as it is, named ${title(overlap.minePath)}.${later}`;
        return `Your version stays as it is.${later}`;
      case "theirs":
        if (overlap.kind === "modified_deleted" || overlap.kind === "renamed_deleted") return "It is deleted, as on GitHub, with your changes to it.";
        if (overlap.kind === "deleted_modified" || overlap.kind === "deleted_renamed") return `GitHub's version comes back, named ${title(overlap.theirsPath)}.`;
        if (renamed) return `GitHub's version replaces yours, named ${title(overlap.theirsPath)}.`;
        return "GitHub's version replaces yours.";
      case "both":
        return "";
      case "merged": {
        const name = renamed ? ` Saved as ${title(overlap.path)}.` : "";
        const prefilled = defaultPick(overlap);
        if (!counts || prefilled?.choice !== "merged" || prefilled.text !== pick.text) return `Merged in the merge view.${name}${later}`;
        const parts = [counts.mine && `${plural(counts.mine, "change")} of yours`, counts.theirs && `${plural(counts.theirs, "change")} from GitHub`];
        return `Merged: ${parts.filter(Boolean).join(" and ") || "the same changes on both sides"}, no clashes.${name}${later}`;
      }
    }
  }

  /** Esc or a click outside: a review with choices in it asks first, as the merge view keeps edits. */
  function close() {
    if (review.busy) return;
    if (review.resolveWithGit) review.resolveWithGit = false;
    else if (review.dirty) discarding = !discarding;
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
              {@const label = pathLabel(overlapName(overlap))}
              <li class="file" class:open={!pick}>
                <div class="name">
                  {@render fileName(overlapName(overlap))}
                  {#if review.reset.includes(overlap.path)}<span class="changed">new or changed</span>{/if}
                </div>
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
                  {#if pick?.choice === "both" && overlap.copy === overlap.theirsPath}
                    Yours stays; GitHub's is kept too, under its own name <code>{overlap.copy}</code>.
                  {:else if pick?.choice === "both"}
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
      {#if discarding}
        <div class="discard" role="alertdialog" aria-label="Discard your choices?">
          <p>Discard your choices for this pull? Nothing has been written; Pull opens the review again.</p>
          <button type="button" class="button" data-autofocus {@attach (node) => node.focus()} onclick={() => (discarding = false)}>
            Keep reviewing
          </button>
          <button type="button" class="button" onclick={() => app.cancelReview()}>Discard</button>
        </div>
      {/if}

      <div class="actions">
        {#if gitOffered}
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
        finish with your editor, <code>git mergetool</code> or the command line.
      </p>
      {#if withMarkers.length}
        <p>git writes conflict markers (<code>&lt;&lt;&lt;&lt;&lt;&lt;&lt;</code>) where the changes clash, in:</p>
        <ul class="paths">
          {#each withMarkers as overlap (overlap.path)}<li class="mono">{overlapName(overlap)}</li>{/each}
        </ul>
      {/if}
      {#if mergedByGit.length}
        <p>git merges {mergedByGit.length === 1 ? "this file" : "these files"} on its own, without markers:</p>
        <ul class="paths">
          {#each mergedByGit as overlap (overlap.path)}<li class="mono">{overlapName(overlap)}</li>{/each}
        </ul>
      {/if}
      {#if precommit.length}
        <p>
          First, your uncommitted changes to {precommit.length === 1 ? "this file are" : "these files are"} committed, so git
          can merge them (other files stay as they are). Abort merge makes them uncommitted again.
        </p>
        <ul class="paths">
          {#each precommit as path (path)}<li class="mono">{path}</li>{/each}
        </ul>
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
          data-autofocus={precommit.length ? undefined : true}
          {@attach (node) => {
            if (!precommit.length) node.focus();
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

  .changed {
    margin-left: 6px;
    font-size: 11px;
    color: var(--color-accent);
  }

  .discard {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-left: 2px solid var(--color-danger);
  }

  .discard p {
    flex: 1 1 16em;
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
