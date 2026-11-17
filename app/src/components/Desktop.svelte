<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import { shortcutLabel } from "../lib/shortcuts";
  import Editor from "./Editor.svelte";
  import EntryHeader from "./EntryHeader.svelte";
  import Sidebar from "./Sidebar.svelte";
  import StatusBar from "./StatusBar.svelte";

  let { app }: { app: AppState } = $props();

  const newKey = $derived(shortcutLabel("new", app.mac));
</script>

<div class="desktop" class:sidebar={app.sidebarOpen}>
  {#if app.sidebarOpen}<Sidebar {app} />{/if}
  <main>
    {#if app.current.file !== null}
      <EntryHeader {app} />
      <Editor {app} />
    {:else if app.entries.length}
      <p class="empty">Pick an entry, or press {newKey} for a new one.</p>
    {:else}
      <p class="empty">No entries yet — {newKey} to start writing.</p>
    {/if}
  </main>
  <StatusBar {app} />
</div>

<style>
  .desktop {
    display: grid;
    grid-template-columns: 1fr;
    grid-template-rows: minmax(0, 1fr) auto;
    height: 100%;
  }

  .desktop.sidebar {
    grid-template-columns: clamp(200px, 22%, 280px) minmax(0, 1fr);
  }

  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .empty {
    margin: auto;
    color: var(--color-muted);
  }
</style>
