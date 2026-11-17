<!-- Placeholder shell: replaced by the UI builder. -->
<script lang="ts">
  import { createEditor } from "./editor";

  function editor(node: HTMLElement) {
    const handle = createEditor(node, {
      doc: "",
      mode: "styled",
      vim: false,
      dark: matchMedia("(prefers-color-scheme: dark)").matches,
      mobile: false,
      onChange() {},
    });
    handle.focus();
    return () => handle.destroy();
  }
</script>

<main {@attach editor}></main>

<style>
  :global(html, body) {
    margin: 0;
    height: 100%;
    background: var(--color-bg);
    color: var(--color-text);
  }

  main {
    height: 100vh;
  }
</style>
