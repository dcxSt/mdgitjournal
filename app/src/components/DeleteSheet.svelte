<!-- Confirms deleting the open entry. An app overlay, so shortcuts can't act behind it. -->
<script lang="ts">
  import { untrack } from "svelte";
  import type { AppState } from "../lib/app.svelte";
  import Sheet from "./Sheet.svelte";

  let { app }: { app: AppState } = $props();

  // The entry the user asked to delete, even if another one opens meanwhile.
  const file = untrack(() => app.current.file);
  let deleting = $state(false);

  const change = $derived(app.sync.changes.find((c) => c.path === file));
  const consequence = $derived(
    change?.kind === "added"
      ? "It has never been committed, so it can't be recovered."
      : change
        ? "Its last committed version stays in the git history; changes since then are lost."
        : "It stays in the git history.",
  );

  function close() {
    if (!deleting) app.closeOverlay();
  }

  async function remove() {
    if (file === null) return;
    deleting = true;
    await app.remove(file);
    deleting = false;
    close();
  }
</script>

<Sheet label="Delete entry" onclose={close}>
  <h2>Delete this entry?</h2>
  <p class="file">{file}</p>
  <p class="muted">{consequence}</p>
  <div class="actions">
    <button type="button" class="button" data-autofocus disabled={deleting} onclick={close}>Cancel</button>
    <button type="button" class="button danger" disabled={deleting || file === null} onclick={remove}>Delete</button>
  </div>
</Sheet>

<style>
  h2 {
    margin: 0 0 8px;
    font-size: 15px;
    font-weight: 500;
  }

  p {
    margin: 0 0 8px;
  }

  .file {
    font-family: var(--font-source);
    font-size: 12px;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
</style>
