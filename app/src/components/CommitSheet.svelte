<!-- The Commit (& Push) sheet: changed files and an empty, required message. -->
<script lang="ts">
  import { untrack } from "svelte";
  import type { AppState } from "../lib/app.svelte";
  import { commitSheetKey } from "../lib/shortcuts";
  import { commitMessageValid, isPushOnly, plural } from "../lib/sync";
  import type { ChangeKind } from "../lib/types";
  import Sheet from "./Sheet.svelte";

  let { app }: { app: AppState } = $props();

  const KIND: Record<ChangeKind, string> = { added: "A", modified: "M", deleted: "D", renamed: "R" };

  let message = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  // Nothing to commit but local commits to push: no message needed. If the backend still wants
  // one (it rejects blank messages before looking for changes), the box appears after all.
  let askMessage = $state(untrack(() => !isPushOnly(app.sync)));

  const view = $derived(app.sync);
  const label = $derived(view.connected ? "Commit & Push" : "Commit");
  const ready = $derived(!busy && (!askMessage || commitMessageValid(message)));

  async function submit() {
    if (!ready) return;
    busy = true;
    error = null;
    const failure = await app.commit(askMessage ? message : "");
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

  function onkeydown(e: KeyboardEvent) {
    const key = commitSheetKey(e);
    if (!key) return;
    e.preventDefault();
    if (key === "submit") void submit();
    else close();
  }
</script>

<Sheet {label} onclose={close}>
  <form
    onsubmit={(e) => {
      e.preventDefault();
      void submit();
    }}
  >
    <h2>{label}</h2>
    {#if view.changes.length}
      <ul class="changes" aria-label="Changed files">
        {#each view.changes as change (change.path)}
          <li>
            <span class="kind {change.kind}" title={change.kind}>{KIND[change.kind]}</span>
            <span class="path">{change.oldPath ? `${change.oldPath} → ${change.path}` : change.path}</span>
          </li>
        {/each}
      </ul>
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

  .changes {
    max-height: 30vh;
    margin: 0 0 12px;
    padding: 0;
    overflow-y: auto;
    list-style: none;
    font-family: var(--font-source);
    font-size: 12px;
  }

  li {
    display: flex;
    gap: 8px;
    padding: 2px 0;
  }

  .kind {
    flex: none;
    width: 1.5ch;
    color: var(--color-muted);
  }

  .kind.added {
    color: var(--color-accent);
  }

  .kind.deleted {
    color: var(--color-danger);
  }

  .path {
    overflow-wrap: anywhere;
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

  @media (max-width: 699px) {
    .actions .button {
      min-height: var(--touch);
      padding: 0 14px;
    }
  }
</style>
