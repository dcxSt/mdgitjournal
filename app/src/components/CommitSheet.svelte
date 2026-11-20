<!-- The Commit (& Push) sheet: the changed files, each with a checkbox (all ticked) and its diff,
     and an empty, required message. -->
<script lang="ts">
  import { untrack } from "svelte";
  import { SvelteMap, SvelteSet } from "svelte/reactivity";
  import { toApiError } from "../lib/api";
  import type { AppState } from "../lib/app.svelte";
  import { commitSheetKey } from "../lib/shortcuts";
  import { commitMessageValid, isPushOnly, plural } from "../lib/sync";
  import type { ChangeKind, FileDiff } from "../lib/types";
  import DiffView from "./DiffView.svelte";
  import Icon from "./Icon.svelte";
  import Sheet from "./Sheet.svelte";

  let { app }: { app: AppState } = $props();

  const KIND: Record<ChangeKind, string> = { added: "A", modified: "M", deleted: "D", renamed: "R" };
  const KIND_NAME: Record<ChangeKind, string> = { added: "new", modified: "changed", deleted: "deleted", renamed: "renamed" };

  let message = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  // Nothing to commit but local commits to push: no message needed. If the backend still wants
  // one (it rejects blank messages before looking for changes), the box appears after all.
  let askMessage = $state(untrack(() => !isPushOnly(app.sync)));
  /** Files left out of this commit; anything else, including files that appear later, is in. */
  const unticked = new SvelteSet<string>();
  const shown = new SvelteSet<string>();
  const diffs = new SvelteMap<string, FileDiff | { error: string } | null>();

  const view = $derived(app.sync);
  const label = $derived(view.connected ? "Commit & Push" : "Commit");
  const ticked = $derived(view.changes.filter((c) => !unticked.has(c.path)).map((c) => c.path));
  const all = $derived(ticked.length === view.changes.length);
  const ready = $derived(
    !busy && (!askMessage || commitMessageValid(message)) && (view.changes.length === 0 || ticked.length > 0),
  );

  async function submit() {
    if (!ready) return;
    busy = true;
    error = null;
    const failure = await app.commit(askMessage ? message : "", view.changes.length ? ticked : []);
    busy = false;
    if (!failure) {
      app.overlay = null;
      return;
    }
    if (failure.kind === "empty_message") askMessage = true;
    error = failure.message;
  }

  function close() {
    if (!busy) app.overlay = null;
  }

  function tick(path: string, on: boolean) {
    if (on) unticked.delete(path);
    else unticked.add(path);
  }

  function tickAll(on: boolean) {
    for (const c of view.changes) tick(c.path, on);
  }

  async function toggleDiff(path: string) {
    if (shown.has(path)) {
      shown.delete(path);
      return;
    }
    shown.add(path);
    if (diffs.has(path)) return;
    diffs.set(path, null);
    try {
      diffs.set(path, await app.api.diff(path));
    } catch (e) {
      diffs.set(path, { error: toApiError(e).message });
    }
  }

  function onkeydown(e: KeyboardEvent) {
    const key = commitSheetKey(e);
    if (!key) return;
    e.preventDefault();
    if (key === "submit") void submit();
    else close();
  }
</script>

<Sheet {label} wide onclose={close}>
  <form
    onsubmit={(e) => {
      e.preventDefault();
      void submit();
    }}
  >
    <h2>{label}</h2>
    {#if view.changes.length}
      {#if view.changes.length > 1}
        <label class="all">
          <input
            type="checkbox"
            checked={all}
            indeterminate={!all && ticked.length > 0}
            disabled={busy}
            onchange={(e) => tickAll(e.currentTarget.checked)}
          />
          <span>{all ? `All ${view.changes.length} changed files` : `${ticked.length} of ${view.changes.length} files`}</span>
        </label>
      {/if}
      <ul class="changes" aria-label="Changed files">
        {#each view.changes as change (change.path)}
          {@const name = change.oldPath ? `${change.oldPath} → ${change.path}` : change.path}
          {@const diff = diffs.get(change.path)}
          <li>
            <div class="row">
              <input
                type="checkbox"
                aria-label={`Commit ${name}`}
                checked={!unticked.has(change.path)}
                disabled={busy}
                onchange={(e) => tick(change.path, e.currentTarget.checked)}
              />
              <span class="kind {change.kind}" title={KIND_NAME[change.kind]}>{KIND[change.kind]}</span>
              <button
                type="button"
                class="path"
                aria-expanded={shown.has(change.path)}
                title={shown.has(change.path) ? "Hide the changes" : "Show the changes"}
                onclick={() => toggleDiff(change.path)}
              >
                <span class="chevron" class:open={shown.has(change.path)}><Icon name="back" size={10} /></span>
                <span class="name">{name}</span>
              </button>
            </div>
            {#if shown.has(change.path)}
              {#if diff === null || diff === undefined}
                <p class="loading muted"><span class="spinner" aria-hidden="true"></span> Loading the changes…</p>
              {:else if "error" in diff}
                <p class="error">{diff.error}</p>
              {:else}
                <DiffView {diff} />
              {/if}
            {/if}
          </li>
        {/each}
      </ul>
      {#if !all}
        <p class="muted small">
          {ticked.length
            ? "Unticked files stay as changes for a later commit."
            : "Tick the files that go into this commit."}
        </p>
      {/if}
    {/if}
    {#if view.connected && view.ahead > 0}
      <p class="muted">
        {view.changes.length
          ? `Also pushes ${plural(view.ahead, "earlier commit")}.`
          : `Nothing new to commit. ${plural(view.ahead, "local commit")} will be pushed.`}
      </p>
    {/if}
    {#if askMessage}
      <!-- data-autofocus when the sheet opens; the attachment when the box appears later. -->
      <textarea
        data-autofocus
        {@attach (node) => node.focus()}
        bind:value={message}
        rows="3"
        aria-label="Commit message"
        placeholder="What changed?"
        spellcheck="true"
        readonly={busy}
        {onkeydown}
      ></textarea>
      <p class="keys muted">Enter to confirm · Shift+Enter for a new line · Esc to cancel</p>
    {/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button type="button" class="button" disabled={busy} onclick={close}>Cancel</button>
      <button type="submit" class="button primary" data-autofocus={askMessage ? undefined : true} disabled={!ready}>
        {#if busy}<span class="spinner" aria-hidden="true"></span>{/if}
        {label}
      </button>
    </div>
  </form>
</Sheet>

<style>
  h2 {
    margin: 0 0 10px;
    font-size: 15px;
    font-weight: 500;
  }

  .all {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 2px;
    color: var(--color-muted);
  }

  .changes {
    max-height: 46vh;
    margin: 0 0 10px;
    padding: 0;
    overflow-y: auto;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 26px;
  }

  input[type="checkbox"] {
    flex: none;
    margin: 0;
  }

  .kind {
    flex: none;
    width: 1.5ch;
    font-family: var(--font-source);
    font-size: 12px;
    color: var(--color-muted);
  }

  .kind.added {
    color: var(--color-accent);
  }

  .kind.deleted {
    color: var(--color-danger);
  }

  .path {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    padding: 2px 6px 2px 2px;
    text-align: left;
  }

  .name {
    font-family: var(--font-source);
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .chevron {
    display: grid;
    flex: none;
    color: var(--color-muted);
    transform: rotate(180deg);
    transition: transform var(--ease);
  }

  .chevron.open {
    transform: rotate(270deg);
  }

  li > :global(.diff),
  li > :global(.note),
  .loading,
  li > .error {
    margin-left: 24px;
  }

  .loading {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 2px;
    margin-bottom: 6px;
  }

  textarea {
    display: block;
    width: 100%;
    resize: vertical;
    font-family: var(--font-ui);
    line-height: 1.45;
  }

  p {
    margin: 0 0 10px;
  }

  .small {
    font-size: 12px;
  }

  .keys {
    margin: 4px 0 0;
    font-size: 11px;
  }

  .error {
    margin-top: 10px;
    color: var(--color-danger);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
  }

  .actions .button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 28px;
  }

  :global([data-layout="phone"]) .row,
  :global([data-layout="phone"]) .all {
    min-height: var(--touch);
  }

  :global([data-layout="phone"]) input[type="checkbox"] {
    width: 22px;
    height: 22px;
  }

  :global([data-layout="phone"]) .path {
    min-height: var(--touch);
  }

  :global([data-layout="phone"]) .actions .button {
    min-height: var(--touch);
    padding: 0 14px;
  }
</style>
