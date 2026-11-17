<!-- Banners (needs-pull, conflicts, errors) and short toasts. -->
<script lang="ts">
  import type { AppState } from "../lib/app.svelte";
  import Icon from "./Icon.svelte";

  let { app }: { app: AppState } = $props();
</script>

{#if app.notices.list.length}
  <div class="notices" class:mobile={app.mobile}>
    {#each app.notices.list as notice (notice.id)}
      <div class="notice" class:error={notice.tone === "error"} role={notice.tone === "error" ? "alert" : "status"}>
        <div class="text">
          <p>{notice.text}</p>
          {#if notice.files?.length}
            <ul>
              {#each notice.files as link (link.file)}
                <li>
                  <button type="button" class="link" onclick={() => app.openEntry(link.file)}>{link.label}</button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
        {#if notice.action}
          {@const action = notice.action}
          <button
            type="button"
            class="action"
            onclick={() => {
              app.notices.dismiss(notice.id);
              action.run();
            }}
          >
            {action.label}
          </button>
        {/if}
        <button type="button" class="dismiss" aria-label="Dismiss" onclick={() => app.notices.dismiss(notice.id)}>
          <Icon name="close" size={12} />
        </button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .notices {
    position: fixed;
    left: 50%;
    bottom: 44px;
    z-index: 20;
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: min(520px, calc(100vw - 24px));
    transform: translateX(-50%);
  }

  .notices.mobile {
    bottom: auto;
    top: calc(52px + env(safe-area-inset-top));
  }

  .notice {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 8px 6px 8px 12px;
    border: 1px solid var(--color-line);
    border-radius: var(--radius);
    background: var(--color-surface);
  }

  .notice.error {
    border-color: color-mix(in srgb, var(--color-danger) 45%, transparent);
  }

  .notice.error p {
    color: var(--color-danger);
  }

  .text {
    flex: 1;
    min-width: 0;
  }

  p {
    margin: 2px 0;
  }

  ul {
    margin: 4px 0 0;
    padding: 0;
    list-style: none;
  }

  .link {
    padding: 1px 0;
    color: var(--color-accent);
    font-family: var(--font-source);
    font-size: 12px;
    text-align: left;
    overflow-wrap: anywhere;
  }

  .link:hover {
    background: none;
    text-decoration: underline;
  }

  .action {
    flex: none;
    color: var(--color-accent);
  }

  .dismiss {
    display: grid;
    place-items: center;
    flex: none;
    width: 24px;
    height: 24px;
    padding: 0;
    color: var(--color-muted);
  }

  .mobile .dismiss,
  .mobile .action {
    min-width: var(--touch);
    min-height: var(--touch);
    margin: -10px 0;
  }
</style>
