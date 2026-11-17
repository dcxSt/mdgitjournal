<!-- Settings → Connect GitHub (PLAN §4): sign in, choose a repository, connect. -->
<script lang="ts">
  import { onMount, untrack } from "svelte";
  import type { AppState } from "../lib/app.svelte";
  import { ConnectFlow } from "../lib/connect.svelte";
  import { plural } from "../lib/sync";

  let { app, ondone }: { app: AppState; ondone: () => void } = $props();

  const PAT_URL = "https://github.com/settings/personal-access-tokens/new";

  const flow = untrack(() => new ConnectFlow(app));
  let token = $state("");
  let fullName = $state(untrack(() => app.sync.repo ?? ""));
  let copied = $state(false);

  const step = $derived(flow.step);
  const placeholderBuild = $derived(app.info?.clientIdPlaceholder ?? false);

  onMount(() => {
    void flow.begin();
    return () => flow.dispose();
  });

  async function copy(code: string) {
    try {
      await navigator.clipboard.writeText(code);
      copied = true;
    } catch {
      flow.error = "Couldn't copy the code. Select it and copy it by hand.";
    }
  }

  function openConflict(file: string) {
    app.overlay = null;
    void app.openEntry(file);
  }
</script>

{#snippet patForm()}
  <form
    class="pat"
    onsubmit={(e) => {
      e.preventDefault();
      void flow.usePat(token, fullName);
    }}
  >
    <p class="muted">
      Create a fine-grained token with access to one repository and the permission
      <em>Contents: read and write</em>.
      <button type="button" class="link" onclick={() => flow.openUrl(PAT_URL)}>Create a token on GitHub</button>
    </p>
    <label>
      <span>Token</span>
      <input
        type="password"
        bind:value={token}
        autocomplete="off"
        spellcheck="false"
        placeholder={app.auth.kind === "pat" ? "Saved; paste a new one to replace it" : "github_pat_…"}
      />
    </label>
    <label>
      <span>Repository</span>
      <input
        type="text"
        bind:value={fullName}
        placeholder="owner/repo"
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
      />
    </label>
    <div class="row">
      <button type="submit" class="button primary" disabled={flow.busy}>Connect</button>
    </div>
  </form>
{/snippet}

<div class="connect">
  {#if step.name === "start"}
    {#if app.sync.connected}
      <p>The GitHub sign-in for <strong>{app.sync.repo}</strong> has expired. Sign in again to keep syncing.</p>
    {/if}
    {#if placeholderBuild}
      <p class="note">
        This build has no GitHub App registered, so “Sign in with GitHub” isn't available. Connect with a
        personal access token instead.
      </p>
      {@render patForm()}
    {:else}
      <p class="muted">
        You approve mdgitjournal on github.com; no password is typed here. You sign in once per device.
      </p>
      <div class="row">
        <button type="button" class="button primary" disabled={flow.busy} onclick={() => flow.signIn()}>
          {#if flow.busy}<span class="spinner" aria-hidden="true"></span>{/if}
          Sign in with GitHub
        </button>
      </div>
      <details>
        <summary>Use a personal access token</summary>
        {@render patForm()}
      </details>
    {/if}
  {:else if step.name === "code"}
    <p>Enter this code on GitHub:</p>
    <div class="code">
      <output aria-label="Your code">{step.userCode}</output>
      <button type="button" class="button" onclick={() => copy(step.userCode)}>{copied ? "Copied" : "Copy"}</button>
    </div>
    <div class="row">
      <button
        type="button"
        class="button primary"
        onclick={() => flow.openUrl(step.verificationUri)}
      >
        Open GitHub
      </button>
      <button type="button" class="button" onclick={() => flow.cancel()}>Cancel</button>
    </div>
    <p class="muted waiting">
      <span class="spinner" aria-hidden="true"></span>
      Waiting for you to approve on GitHub…
    </p>
  {:else if step.name === "repos"}
    <p>
      Signed in as <strong>@{app.auth.login}</strong>.
      <button type="button" class="link" onclick={() => flow.signOut()}>Sign out</button>
    </p>
    {#if app.auth.kind === "pat"}
      {@render patForm()}
    {:else}
      <p>Choose the repository for this journal:</p>
      {#if step.repos === null}
        <p class="muted">Loading repositories…</p>
      {:else if step.repos.length}
        <ul class="repos">
          {#each step.repos as repo (repo.fullName)}
            <li>
              <button type="button" onclick={() => flow.connect(repo.fullName)}>
                <span>{repo.fullName}</span>
                <span class="muted">{repo.private ? "private" : "public"}</span>
              </button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="muted">mdgitjournal can't see any repositories yet.</p>
      {/if}
      <div class="row wrap">
        <button
          type="button"
          class="button"
          disabled={!flow.urls}
          onclick={() => flow.urls && flow.openUrl(flow.urls.newRepo)}
        >
          Create a private repo on GitHub
        </button>
        <button
          type="button"
          class="button"
          disabled={!flow.urls}
          onclick={() => flow.urls && flow.openUrl(flow.urls.install)}
        >
          Give mdgitjournal access to a repo
        </button>
        <button type="button" class="button" onclick={() => flow.showRepos()}>Refresh</button>
      </div>
      <p class="muted">mdgitjournal can only read and write the contents of the repositories you give it.</p>
      <details>
        <summary>Use a personal access token instead</summary>
        {@render patForm()}
      </details>
    {/if}
  {:else if step.name === "connecting"}
    <p class="waiting"><span class="spinner" aria-hidden="true"></span> Connecting to {step.fullName}…</p>
  {:else}
    {#if step.result}
      <p>Connected to <strong>{step.fullName}</strong>.</p>
      {#if step.result.updated.length}
        <p class="muted">{plural(step.result.updated.length, "file")} came from GitHub.</p>
      {/if}
      {#if step.result.conflicts.length}
        <p>
          {plural(step.result.conflicts.length, "file")} existed on both sides. GitHub's version keeps the name; yours
          is saved next to it:
        </p>
        <ul class="conflicts">
          {#each step.result.conflicts as conflict (conflict.path)}
            <li>
              <button type="button" class="link" onclick={() => openConflict(conflict.copy ?? conflict.path)}>
                {conflict.copy ?? conflict.path}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    {:else}
      <p>Signed in again as <strong>@{app.auth.login}</strong>.</p>
    {/if}
    <div class="row">
      <button type="button" class="button primary" onclick={ondone}>Done</button>
    </div>
  {/if}
  {#if flow.error}<p class="error" role="alert">{flow.error}</p>{/if}
</div>

<style>
  .connect {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  p {
    margin: 0;
  }

  .note {
    padding: 8px 10px;
    border-left: 2px solid var(--color-accent);
    background: var(--color-accent-soft);
  }

  .row {
    display: flex;
    gap: 8px;
  }

  .row.wrap {
    flex-wrap: wrap;
  }

  .button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .code {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  output {
    font-family: var(--font-source);
    font-size: 28px;
    letter-spacing: 0.08em;
    user-select: all;
    -webkit-user-select: all;
  }

  .waiting {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .repos,
  .conflicts {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .repos {
    border: 1px solid var(--color-line);
    border-radius: var(--radius);
  }

  .repos li + li {
    border-top: 1px solid var(--color-line);
  }

  .repos button {
    display: flex;
    justify-content: space-between;
    width: 100%;
    padding: 8px 10px;
    border-radius: 0;
    text-align: left;
  }

  .pat {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 8px;
  }

  label {
    display: grid;
    grid-template-columns: 90px 1fr;
    align-items: center;
    gap: 8px;
  }

  summary {
    color: var(--color-muted);
  }

  .link {
    padding: 0;
    color: var(--color-accent);
  }

  .link:hover {
    background: none;
    text-decoration: underline;
  }

  .conflicts .link {
    font-family: var(--font-source);
    font-size: 12px;
    text-align: left;
    overflow-wrap: anywhere;
  }

  .error {
    color: var(--color-danger);
  }

  :global([data-layout="phone"]) .button,
  :global([data-layout="phone"]) .repos button {
    min-height: var(--touch);
  }

  :global([data-layout="phone"]) label {
    grid-template-columns: 1fr;
    gap: 4px;
  }
</style>
