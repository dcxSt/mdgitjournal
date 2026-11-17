# Privacy

**mdgitjournal collects no data.**

Your journal lives on your device and, if you connect one, in the GitHub repository you choose. There is no mdgitjournal server. Nobody else receives a copy, the developer included.

- **No tracking.** No analytics, telemetry, crash reports, advertising or accounts.
- **Network.** Without GitHub, the app makes no network connections. With GitHub connected, it talks only to `github.com` and `api.github.com`, and only to:
  - sign you in, and renew that sign-in;
  - read your GitHub user name for commit authorship, and list the repositories you installed the app on;
  - fetch, which it does on launch, when its window regains focus, and every 5 minutes by default (adjustable or off in Settings);
  - pull, push, or connect a repository when you ask.
- **Tokens** stay on your device, in the Keychain on macOS and iOS or the Secret Service on Linux. Without a Secret Service, they go in a file only your user can read. They are never written into your journal or its git configuration.
- **Images** you add are re-encoded on your device, and all their metadata, including location, is removed.

GitHub's handling of your repository and account is covered by the [GitHub Privacy Statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement). If you install from the App Store, Apple's privacy policy covers the store.

The website [mdgitjournal.stephenfay.xyz](https://mdgitjournal.stephenfay.xyz) sets no cookies and loads nothing from third parties. It is hosted on GitHub Pages, which may log visitors' IP addresses for security.

Questions: [open an issue](https://github.com/dcxSt/mdgitjournal/issues).

Last updated: 16 November 2026.
