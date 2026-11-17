# GitHub App

mdgitjournal signs in to GitHub through a GitHub App using the OAuth device flow, so it needs no client secret and runs no server. Each user installs the app on one repository, their journal; the app's tokens reach only the repositories it is installed on.

This page covers:

1. [Registering the app](#1-register-the-app), done once by the maintainer.
2. [Building with its client ID](#2-build-with-the-client-id).
3. [Installing it on a journal repository](#3-install-it-on-a-journal-repository), done by each user.
4. [The personal access token alternative](#4-alternative-a-fine-grained-personal-access-token).

## 1. Register the app

Signed in as `dcxSt`, open <https://github.com/settings/apps/new> and fill in the form from top to bottom:

| Field | Value |
|---|---|
| GitHub App name | `mdgitjournal` |
| Description | `A minimal markdown journal synced with git. It needs access to one repository: your journal.` |
| Homepage URL | `https://mdgitjournal.stephenfay.xyz` |
| **Identifying and authorizing users** | |
| Callback URL | Leave empty. It is optional, and GitHub ignores it for device flow. |
| Expire user authorization tokens | **On** (the default). Access tokens last 8 hours and refresh tokens 6 months; the app refreshes silently. |
| Request user authorization (OAuth) during installation | **Off** |
| Enable Device Flow | **On** |
| **Post installation** | |
| Setup URL | Leave empty. |
| Redirect on update | Off |
| **Webhook** | |
| Active | **Off**. There is no server to receive events. |
| **Permissions › Repository permissions** | |
| Contents | **Read and write** |
| Metadata | **Read-only** (selected automatically; GitHub requires it) |
| Everything else | No access |
| **Permissions › Organization / Account permissions** | No access |
| **Where can this GitHub App be installed?** | **Any account** |

Choose **Any account**, or the release builds won't work for anyone but you. This grants you nothing: each user's tokens stay on their own device and reach only the repositories they install the app on. **Only on this account** suits a private fork.

Click **Create GitHub App**. On the app's **General** page:

- Copy the **Client ID**, which starts with `Iv`; the numeric *App ID* is something else. The Client ID is public and gets compiled into the app, so it needs no protection.
- Do **not** generate a client secret or a private key. Device flow and token refresh work without them.
- Optionally, under *Display information*, upload `app/src-tauri/icons/icon.png` as the logo.

The name is part of the app's URL, `https://github.com/apps/mdgitjournal`. If `mdgitjournal` is taken and you pick another name, use the slug from that URL as `MDGJ_GITHUB_APP_SLUG` below.

## 2. Build with the client ID

The client ID is read at compile time from `MDGJ_GITHUB_CLIENT_ID`. Without one, the app uses a placeholder and only personal access tokens work.

- **For every build** (recommended): commit the ID in `.cargo/config.toml`. Local builds, CI, release builds and the AUR package then all get it.
  ```toml
  [env]
  MACOSX_DEPLOYMENT_TARGET = "11.0"
  MDGJ_GITHUB_CLIENT_ID = "Iv23…"
  MDGJ_GITHUB_APP_SLUG = "<slug>"   # only if not "mdgitjournal"
  ```
  Leave `DEFAULT_CLIENT_ID` in `crates/mdgj-github/src/client.rs` as it is. The app compares the client ID with that placeholder to tell whether a GitHub App is configured.
- **For one build**, or for a fork with its own app, set the environment variable. It takes precedence over `.cargo/config.toml`:
  ```sh
  MDGJ_GITHUB_CLIENT_ID=Iv23… pnpm -C app tauri build
  ```
- **For release builds only**, set a repository *variable*. A secret isn't needed, since the ID is public. The release workflow warns when no client ID is set anywhere.
  ```sh
  gh variable set MDGJ_GITHUB_CLIENT_ID --repo dcxSt/mdgitjournal --body Iv23…
  ```

## 3. Install it on a journal repository

Each user does this once, from **Settings › Connect GitHub** in the app:

1. **Sign in with GitHub.** The app shows a code such as `ABCD-1234`. Click **Open GitHub**, enter the code and approve.
2. **Have a repository.** If you don't have one yet, **New repository** opens <https://github.com/new?name=journal&visibility=private>. Private is recommended.
3. **Install the app.** **Install** opens <https://github.com/apps/mdgitjournal/installations/new>. Pick your account, choose **Only select repositories**, select the journal repository and click **Install**. Avoid *All repositories*: the app needs only one.
4. Back in the app, pick the repository and **Connect**. An empty repository receives your local history; an existing one is merged with it.

For a repository owned by an organization, an organization owner installs the app, or approves your request to install it.

To change which repository the app can see, or to uninstall it, go to <https://github.com/settings/installations>, then **mdgitjournal › Configure**. To revoke the sign-in itself, go to <https://github.com/settings/apps/authorizations>. **Disconnect** in the app removes the token and the remote, and keeps your files.

## 4. Alternative: a fine-grained personal access token

Use this for source builds without a registered GitHub App, or to test before the app exists.

1. Open <https://github.com/settings/personal-access-tokens/new>.
2. Fill in the form:
   - **Token name:** e.g. `mdgitjournal (laptop)`.
   - **Expiration:** your choice. When the token expires, paste a new one.
   - **Resource owner:** the owner of the journal repository.
   - **Repository access:** **Only select repositories**, then the journal repository.
   - **Permissions › Repository permissions:** set **Contents** to **Read and write**. **Metadata: Read-only** is added automatically. Leave everything else at *No access*.
3. Click **Generate token** and copy it (`github_pat_…`).
4. In the app, go to **Settings › Connect GitHub › Advanced**. Paste the token, type `owner/repo` and **Connect**.

The token is stored like the app's own tokens: in the Keychain or Secret Service, never in the repository or `.git/config`.
