<!-- Formatting keys above the phone keyboard. Pressing them must not move focus out of the editor,
     or the keyboard would close. -->
<script lang="ts">
  import type { EditorHandle } from "../editor";
  import Icon from "./Icon.svelte";

  let { editor, onphoto }: { editor: EditorHandle; onphoto: () => void } = $props();

  const KEYS: { label: string; name: string; style?: string; run: (e: EditorHandle) => void }[] = [
    { label: "#", name: "Heading", run: (e) => e.toggleLinePrefix("# ") },
    { label: "B", name: "Bold", style: "bold", run: (e) => e.wrapSelection("**", "**") },
    { label: "I", name: "Italic", style: "italic", run: (e) => e.wrapSelection("_", "_") },
    { label: "-", name: "List item", run: (e) => e.toggleLinePrefix("- ") },
    { label: "[ ]", name: "Task", run: (e) => e.toggleLinePrefix("- [ ] ") },
  ];
</script>

<div class="accessory" role="toolbar" aria-label="Formatting" tabindex="-1" onmousedown={(e) => e.preventDefault()}>
  {#each KEYS as key (key.name)}
    <button type="button" tabindex="-1" class={key.style} aria-label={key.name} onclick={() => key.run(editor)}>
      {key.label}
    </button>
  {/each}
  <button type="button" tabindex="-1" aria-label="Link" onclick={() => editor.wrapSelection("[", "]()")}>
    <Icon name="link" size={18} />
  </button>
  <button type="button" tabindex="-1" aria-label="Add photo" onclick={onphoto}>
    <Icon name="camera" size={18} />
  </button>
  <span class="grow"></span>
  <button
    type="button"
    tabindex="-1"
    class="done"
    onclick={() => (document.activeElement as HTMLElement | null)?.blur()}
  >
    Done
  </button>
</div>

<style>
  .accessory {
    display: flex;
    align-items: center;
    flex: none;
    padding: 0 max(4px, env(safe-area-inset-right)) var(--safe-bottom) max(4px, env(safe-area-inset-left));
    border-top: 1px solid var(--color-line);
    background: var(--color-surface);
    outline: none;
  }

  button {
    display: grid;
    place-items: center;
    min-width: var(--touch);
    height: var(--touch);
    padding: 0 6px;
    font-family: var(--font-source);
    font-size: 15px;
    color: var(--color-text);
  }

  .bold {
    font-family: var(--font-styled);
    font-weight: bold;
  }

  .italic {
    font-family: var(--font-styled);
    font-style: italic;
  }

  .done {
    font-family: var(--font-ui);
    color: var(--color-accent);
  }
</style>
