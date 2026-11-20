<!-- A modal sheet on the native <dialog>: focus stays inside, Esc and the backdrop close it. -->
<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    label,
    onclose,
    wide = false,
    children,
  }: { label: string; onclose: () => void; wide?: boolean; children: Snippet } = $props();

  // Only a press that starts and ends on the backdrop closes; a text selection dragged out of the
  // sheet must not throw away what was typed.
  let pressedBackdrop = false;

  function open(dialog: HTMLDialogElement) {
    dialog.showModal();
    const target = dialog.querySelector<HTMLElement>("[data-autofocus]");
    if (target) target.focus();
    // On the phone, nothing is chosen for the user: no ring on the first button, no keyboard.
    else if (document.documentElement.dataset.layout === "phone") dialog.focus();
    return () => dialog.close();
  }
</script>

<dialog
  {@attach open}
  tabindex="-1"
  class:wide
  aria-label={label}
  oncancel={(e) => {
    e.preventDefault();
    onclose();
  }}
  onpointerdown={(e) => (pressedBackdrop = e.target === e.currentTarget)}
  onclick={(e) => {
    if (pressedBackdrop && e.target === e.currentTarget) onclose();
  }}
>
  <div class="body">
    {@render children()}
  </div>
</dialog>

<style>
  dialog {
    width: min(460px, calc(100vw - 32px));
    max-height: calc(100vh - 96px);
    margin: 12vh auto auto;
    padding: 0;
    overflow: auto;
    overscroll-behavior: contain;
    border: 1px solid var(--color-line);
    border-radius: 10px;
    background: var(--color-bg);
    color: var(--color-text);
    box-shadow: var(--shadow-sheet);
    animation: sheet-in 100ms ease-out;
  }

  dialog:focus {
    outline: none;
  }

  dialog.wide {
    width: min(620px, calc(100vw - 32px));
    margin-top: 8vh;
  }

  dialog::backdrop {
    background: rgb(0 0 0 / 0.22);
  }

  .body {
    padding: 16px 18px;
  }

  /* On the phone a sheet hangs from the top, clear of the keyboard; wide ones take the screen. */
  :global([data-layout="phone"]) dialog {
    width: 100vw;
    max-width: 100vw;
    margin: 0 0 auto;
    border-width: 0 0 1px;
    border-radius: 0 0 12px 12px;
    max-height: 100%;
  }

  :global([data-layout="phone"]) dialog.wide {
    height: 100%;
    border-radius: 0;
  }

  :global([data-layout="phone"]) .body {
    padding: calc(12px + env(safe-area-inset-top)) 16px calc(16px + env(safe-area-inset-bottom));
  }

  @keyframes sheet-in {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    dialog {
      animation: none;
    }
  }
</style>
