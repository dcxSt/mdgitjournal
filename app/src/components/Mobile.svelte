<!-- The phone layout: the entry list, or the open entry. It follows the visual viewport, so
     when the keyboard is up everything (including the accessory bar) sits above it. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import MobileEditor from "./MobileEditor.svelte";
  import MobileList from "./MobileList.svelte";

  let { app }: { app: AppState } = $props();

  let viewport = $state({ top: 0, height: window.innerHeight, keyboard: false });

  function track() {
    const vv = window.visualViewport;
    if (!vv) return;
    const update = () => {
      // The keyboard is the only thing that shrinks the visual viewport by this much.
      viewport = { top: vv.offsetTop, height: vv.height, keyboard: window.innerHeight - vv.height > 120 };
    };
    update();
    vv.addEventListener("resize", update);
    vv.addEventListener("scroll", update);
    return () => {
      vv.removeEventListener("resize", update);
      vv.removeEventListener("scroll", update);
    };
  }
</script>

<div
  class="mobile"
  class:keyboard={viewport.keyboard}
  style:top="{viewport.top}px"
  style:height="{viewport.height}px"
  {@attach track}
>
  {#if app.current.file === null}
    <MobileList {app} />
  {:else}
    <MobileEditor {app} />
  {/if}
</div>

<style>
  .mobile {
    position: fixed;
    left: 0;
    right: 0;
    display: flex;
    flex-direction: column;
    --row-height: var(--touch);
    --sync-height: var(--touch);
    --icon-button: var(--touch);
    --menu-row: var(--touch);
    --safe-bottom: env(safe-area-inset-bottom);
  }

  /* The keyboard covers the home indicator, so nothing needs to clear it. */
  .mobile.keyboard {
    --safe-bottom: 0px;
  }

  .mobile :global(.bar) {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: none;
    padding: env(safe-area-inset-top) max(6px, env(safe-area-inset-right)) 0 max(6px, env(safe-area-inset-left));
    border-bottom: 1px solid var(--color-line);
  }

  .mobile :global(.grow) {
    flex: 1;
  }
</style>
