<script lang="ts">
  import { imageLink, type EditorHandle } from "../editor";
  import type { AppState } from "../lib/app.svelte";
  import { entryTimestamp } from "../lib/journal";
  import AccessoryBar from "./AccessoryBar.svelte";
  import Editor from "./Editor.svelte";
  import EntryMenu from "./EntryMenu.svelte";
  import Icon from "./Icon.svelte";
  import SaveState from "./SaveState.svelte";
  import SyncButtons from "./SyncButtons.svelte";
  import TitleField from "./TitleField.svelte";

  let { app }: { app: AppState } = $props();

  let editor = $state.raw<EditorHandle>();
  let editing = $state(false);
  let photoInput = $state<HTMLInputElement>();
  const stamp = $derived(app.currentMeta && entryTimestamp(app.currentMeta));

  async function addPhotos(input: HTMLInputElement) {
    const files = [...(input.files ?? [])];
    input.value = "";
    const file = app.current.file;
    const paths = await app.importImages(files);
    if (paths.length && editor && app.current.file === file) {
      editor.insertAtCursor(paths.map(imageLink).join("\n"));
    }
  }
</script>

<header class="bar">
  <button type="button" class="back" onclick={() => app.back()}>
    <Icon name="back" size={18} />
    <span>Entries</span>
  </button>
  <span class="grow"></span>
  <SyncButtons {app} compact />
</header>

<div class="head">
  {#if stamp}<span class="when">{stamp}</span>{/if}
  <TitleField {app} />
  <SaveState current={app.current} />
  <EntryMenu {app} />
</div>

<div class="body" class:editing onfocusin={() => (editing = true)} onfocusout={() => (editing = false)}>
  <Editor {app} bind:handle={editor} />
</div>

{#if editing && editor}
  <AccessoryBar {editor} onphoto={() => photoInput?.click()} />
{/if}
<!-- Outside the bar: picking a photo blurs the editor, which hides the bar. -->
<input
  bind:this={photoInput}
  class="visually-hidden"
  type="file"
  accept="image/*"
  multiple
  tabindex="-1"
  aria-hidden="true"
  onchange={(e) => addPhotos(e.currentTarget)}
/>

<style>
  .back {
    display: flex;
    align-items: center;
    gap: 2px;
    min-height: var(--touch);
    padding: 0 8px 0 2px;
    color: var(--color-accent);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
    padding: 2px max(4px, env(safe-area-inset-right)) 2px max(14px, env(safe-area-inset-left));
    border-bottom: 1px solid var(--color-line);
  }

  .when {
    flex: none;
    color: var(--color-muted);
    font-variant-numeric: tabular-nums;
  }

  .body {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  /* While editing, the accessory bar below takes care of the home indicator. */
  .body:not(.editing) {
    padding-bottom: var(--safe-bottom);
  }
</style>
