<!-- The merge view (PLAN §5): one editable document, prefilled with the merge of both versions,
     over the whole window. Done hands the text to the review; Cancel leaves its choice alone. -->
<script lang="ts">
  import { untrack } from "svelte";
  import { createMergeEditor, type MergeEditorHandle } from "../editor/merge";
  import type { AppState } from "../lib/app.svelte";
  import { pathLabel } from "../lib/journal";
  import type { Review } from "../lib/review.svelte";
  import { plural } from "../lib/sync";

  let { app, review }: { app: AppState; review: Review } = $props();

  const path = untrack(() => review.merging!);
  const overlap = untrack(() => review.overlap(path)!);
  const label = pathLabel(path);

  let editor = $state.raw<MergeEditorHandle>();
  let unresolved = $state(0);
  let edits = $state(0);

  /** Opens the dialog, then the editor in it: CodeMirror measures its lines once they are laid out. */
  function open(dialog: HTMLDialogElement) {
    dialog.showModal();
    const host = dialog.querySelector<HTMLElement>(".editor")!;
    const created = untrack(() =>
      createMergeEditor(host, {
        chunks: overlap.suggestion!.chunks,
        dark: app.dark,
        mobile: app.mobile,
        snapshot: review.snapshot(path),
        onChange: (_doc, left) => {
          unresolved = left;
          edits = created.undoDepth();
        },
      }),
    );
    editor = created;
    unresolved = created.unresolved();
    // On the phone, no field takes focus (and opens the keyboard) until the user taps one.
    if (app.mobile) dialog.focus();
    else created.focus();
    return () => {
      created.destroy();
      editor = undefined;
      dialog.close();
    };
  }

  $effect(() => editor?.setDark(app.dark));

  function done() {
    if (!editor || unresolved > 0) return;
    app.closeMerge({ text: editor.getDoc(), snapshot: editor.snapshot() });
  }

  function cancel() {
    app.closeMerge();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.isComposing) {
      e.preventDefault();
      done();
    }
  }
</script>

<dialog
  {@attach open}
  tabindex="-1"
  class="merge"
  class:phone={app.mobile}
  aria-label={`Merge ${label.title}`}
  oncancel={(e) => {
    e.preventDefault();
    // Esc leaves only while nothing was edited; the Cancel button always does.
    if (edits === 0) cancel();
  }}
  {onkeydown}
>
  <header>
    <button type="button" class="button" onclick={cancel}>Cancel</button>
    <div class="title">
      <h2 class:mono={!label.when}>{label.title}</h2>
      {#if label.when}<span class="when">{label.when}</span>{/if}
    </div>
    <div class="tools">
      <span class="legend" aria-hidden="true">
        <span class="key mine">yours</span>
        <span class="key theirs">GitHub's</span>
      </span>
      <span class="left" class:open={unresolved > 0} aria-live="polite">
        {unresolved ? `${plural(unresolved, "clash")} left` : "No clashes left"}
      </span>
      <button type="button" class="button" disabled={edits === 0} onclick={() => editor?.undo()}>Undo</button>
    </div>
    <button
      type="button"
      class="button primary"
      disabled={unresolved > 0}
      title={unresolved > 0 ? "Choose a version for every clash first" : "Use this text"}
      onclick={done}
    >
      Done
    </button>
  </header>
  <p class="help">
    Changes from both versions are already in, tinted by where they came from. Where both changed the same lines,
    choose a version. You can edit anything.
  </p>
  <div class="editor"></div>
</dialog>

<style>
  dialog.merge {
    width: 100vw;
    max-width: 100vw;
    height: 100%;
    max-height: 100%;
    margin: 0;
    padding: 0;
    border: none;
    background: var(--color-bg);
    color: var(--color-text);
  }

  dialog.merge[open] {
    display: flex;
    flex-direction: column;
  }

  dialog.merge:focus {
    outline: none;
  }

  dialog::backdrop {
    background: var(--color-bg);
  }

  header {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: none;
    padding: 8px 12px;
    border-bottom: 1px solid var(--color-line);
  }

  .title {
    display: flex;
    flex: 1;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }

  h2 {
    margin: 0;
    overflow: hidden;
    font-family: var(--font-styled);
    font-size: 17px;
    font-weight: 400;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  h2.mono {
    font-family: var(--font-source);
    font-size: 13px;
  }

  .when {
    flex: none;
    color: var(--color-muted);
    font-variant-numeric: tabular-nums;
  }

  .tools {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .legend {
    display: flex;
    gap: 10px;
    margin-right: 4px;
    color: var(--color-muted);
    font-size: 12px;
  }

  .key {
    padding: 0 6px;
    border-radius: 3px;
  }

  .key.mine {
    background: color-mix(in srgb, var(--color-accent) 12%, transparent);
    box-shadow: inset 2px 0 color-mix(in srgb, var(--color-accent) 70%, transparent);
  }

  .key.theirs {
    background: color-mix(in srgb, var(--color-theirs) 14%, transparent);
    box-shadow: inset 2px 0 color-mix(in srgb, var(--color-theirs) 80%, transparent);
  }

  .left {
    min-width: 7.5em;
    color: var(--color-muted);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .left.open {
    color: var(--color-accent);
  }

  .button {
    min-height: 28px;
  }

  .help {
    flex: none;
    margin: 0;
    padding: 6px 12px;
    border-bottom: 1px solid var(--color-line);
    color: var(--color-muted);
    font-size: 12px;
  }

  .editor {
    display: grid;
    flex: 1;
    min-height: 0;
  }

  .editor > :global(.cm-editor) {
    min-height: 0;
  }

  /* The phone: Cancel, title and Done, then the legend, what is left and Undo; clear of the
     notch and the home indicator. */
  .phone header {
    flex-wrap: wrap;
    row-gap: 4px;
    column-gap: 8px;
    padding: calc(4px + env(safe-area-inset-top)) max(8px, env(safe-area-inset-right)) 4px
      max(8px, env(safe-area-inset-left));
  }

  .phone .title {
    flex-direction: column;
    align-items: center;
    gap: 0;
    text-align: center;
  }

  .phone .title h2 {
    max-width: 100%;
  }

  .phone .when {
    font-size: 12px;
  }

  .phone .tools {
    order: 3;
    flex-basis: 100%;
  }

  .phone .left {
    flex: 1;
    min-width: 0;
  }

  .phone .help {
    display: none;
  }

  .phone .button {
    min-height: var(--touch);
    padding: 0 14px;
  }

  .phone .editor {
    padding-bottom: env(safe-area-inset-bottom);
  }
</style>
