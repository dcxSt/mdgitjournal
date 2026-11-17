<!-- The "…" menu in the entry header: delete (confirmed in DeleteSheet), and the mode switch on the phone. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import Icon from "./Icon.svelte";

  let { app }: { app: AppState } = $props();

  let open = $state(false);

  function closeOnOutside(node: HTMLElement) {
    const onpointerdown = (e: PointerEvent) => {
      if (!node.contains(e.target as Node)) open = false;
    };
    document.addEventListener("pointerdown", onpointerdown);
    return () => document.removeEventListener("pointerdown", onpointerdown);
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
          app.overlay = "delete";
        }}
      >
        Delete entry…
      </button>
    </div>
  {/if}
</div>

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
</style>
