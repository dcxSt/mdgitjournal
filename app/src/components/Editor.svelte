<!-- Hosts the CodeMirror editor for the open entry and keeps it in step with the app state. -->
<script lang="ts">
  import { untrack } from "svelte";
  import { createEditor, type EditorHandle } from "../editor";
  import type { AppState } from "../lib/app.svelte";

  let { app, handle = $bindable() }: { app: AppState; handle?: EditorHandle } = $props();

  let editor = $state.raw<EditorHandle>();
  /** The `current.version` whose text the editor shows. */
  let shown = 0;
  const vim = $derived(app.settings.vim);

  function mount(node: HTMLElement) {
    const created = untrack(() => {
      shown = app.current.version;
      return createEditor(node, {
        doc: app.current.content,
        mode: app.mode,
        vim: app.settings.vim,
        dark: app.dark,
        mobile: app.mobile,
        onChange: (doc) => app.current.edit(doc),
        onSave: () => void app.current.flush().catch((e) => app.showError(e)),
        onImageFiles: (files) => app.importImages(files),
        onImagesUnlinked: (paths) => app.imagesUnlinked(paths),
        resolveImageUrl: (relPath) => app.imageUrl(relPath),
      });
    });
    editor = handle = created;
    return () => {
      created.destroy();
      editor = handle = undefined;
    };
  }

  // Another entry was opened, or the open one was reloaded from disk.
  $effect(() => {
    const version = app.current.version;
    if (editor && version !== shown) {
      shown = version;
      // The same entry reloaded after a pull or an edit elsewhere keeps its cursor.
      editor.setDoc(untrack(() => app.current.content), { keepCursor: app.current.reloaded });
    }
  });
  $effect(() => editor?.setReadOnly(app.readOnly));
  $effect(() => editor?.setMode(app.mode));
  $effect(() => editor?.setVim(vim));
  $effect(() => editor?.setDark(app.dark));
  $effect(() => {
    if (editor && app.focusEditor) {
      app.focusEditor = false;
      editor.focus();
    }
  });
</script>

<div class="editor" {@attach mount}></div>

<style>
  .editor {
    display: grid;
    flex: 1;
    min-height: 0;
  }

  .editor > :global(.cm-editor) {
    min-height: 0;
  }
</style>
