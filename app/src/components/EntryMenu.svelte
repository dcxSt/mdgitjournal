<!-- The "…" menu in the entry header: delete (with a confirm), and the mode switch on the phone. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import Icon from "./Icon.svelte";
  import Sheet from "./Sheet.svelte";

  let { app }: { app: AppState } = $props();

  let open = $state(false);
  let confirming = $state(false);
  let deleting = $state(false);

  const file = $derived(app.current.file);
  const change = $derived(app.sync.changes.find((c) => c.path === file));
  const consequence = $derived(
    change?.kind === "added"
      ? "It has never been committed, so it can't be recovered."
      : change
        ? "Its last committed version stays in the git history; changes since then are lost."
        : "It stays in the git history.",
  );

  function closeOnOutside(node: HTMLElement) {
    const onpointerdown = (e: PointerEvent) => {
      if (!node.contains(e.target as Node)) open = false;
    };
    document.addEventListener("pointerdown", onpointerdown);
    return () => document.removeEventListener("pointerdown", onpointerdown);
  }

  async function remove() {
    deleting = true;
    await app.remove();
    deleting = false;
    confirming = false;
  }
</script>

<div class="wrap" {@attach closeOnOutside}>
  <button
    type="button"
    class="toggle"
    aria-label="Entry actions"
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={() => (open = !open)}
  >
    <Icon name="more" />
  </button>
  {#if open}
    <div
      class="menu"
      role="menu"
      tabindex="-1"
      onkeydown={(e) => {
        if (e.key === "Escape") open = false;
      }}
    >
      {#if app.mobile}
        <button
          type="button"
          role="menuitem"
          onclick={() => {
            app.mode = app.mode === "source" ? "styled" : "source";
            open = false;
          }}
        >
          {app.mode === "source" ? "Styled view" : "Source view"}
        </button>
      {/if}
      <button
        type="button"
        role="menuitem"
        class="danger"
        onclick={() => {
          open = false;
          confirming = true;
        }}
      >
        Delete entry…
      </button>
    </div>
  {/if}
</div>

{#if confirming && file !== null}
  <Sheet label="Delete entry" onclose={() => !deleting && (confirming = false)}>
    <h2>Delete this entry?</h2>
    <p class="file">{file}</p>
    <p class="muted">{consequence}</p>
    <div class="actions">
      <button type="button" class="button" data-autofocus disabled={deleting} onclick={() => (confirming = false)}>
        Cancel
      </button>
      <button type="button" class="button danger" disabled={deleting} onclick={remove}>Delete</button>
    </div>
  </Sheet>
{/if}

<style>
  .wrap {
    position: relative;
    flex: none;
  }

  .toggle {
    display: grid;
    place-items: center;
    width: var(--icon-button, 26px);
    height: var(--icon-button, 26px);
    padding: 0;
    color: var(--color-muted);
  }

  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    z-index: 10;
    display: flex;
    flex-direction: column;
    min-width: 160px;
    padding: 4px;
    border: 1px solid var(--color-line);
    border-radius: var(--radius);
    background: var(--color-bg);
    box-shadow: var(--shadow-sheet);
  }

  .menu button {
    padding: 6px 10px;
    min-height: var(--menu-row, 0);
    text-align: left;
  }

  .danger {
    color: var(--color-danger);
  }

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
