# Releasing

1. [Set the version and tag it](#1-version-and-tag).
2. [Publish the GitHub Release](#2-github-release).
3. [Update the AUR packages](#3-aur).
4. Optionally, [ship to the App Stores](#5-mac-app-store-ios-app-store-and-testflight).

Before the first release, do the [one-time GitHub setup](#4-one-time-setup-github).

## 1. Version and tag

The version lives in several files. Set the same `X.Y.Z` in all of them:

- `Cargo.toml`: `[workspace.package] version`
- `app/package.json`: `version`
- `app/src-tauri/tauri.conf.json`: `version`
- `packaging/linux/xyz.stephenfay.mdgitjournal.metainfo.xml`: add `<release version="X.Y.Z" date="YYYY-MM-DD"/>` as the first entry in `<releases>`
- `packaging/aur/PKGBUILD` and `packaging/aur/mdgitjournal-bin/PKGBUILD`: `pkgver=X.Y.Z`, `pkgrel=1`

```sh
cargo update --workspace          # refresh the workspace versions in Cargo.lock
git commit -am "Release vX.Y.Z"
git tag -a vX.Y.Z -m "mdgitjournal X.Y.Z"
git push origin main vX.Y.Z
```

`packaging/THIRD-PARTY-NOTICES.md` lists the licences of everything compiled into the app, and CI fails when it is out of date. After changing dependencies, regenerate it with `packaging/notices/generate.sh` and commit it. The script needs jq and cargo-about (`cargo install cargo-about --version 0.9.3 --locked --features cli`).

## 2. GitHub Release

Pushing the tag runs `.github/workflows/release.yml`, which:

1. Fails if the tag doesn't match the versions above, or if `Cargo.lock` is out of date.
2. Creates a **draft** release with generated notes.
3. Builds a universal macOS `.dmg` and the Linux AppImage, `.deb` and `.rpm`, and attaches them.

Check the assets, edit the notes and click **Publish**. If a job fails, delete the draft and the tag (`gh release delete vX.Y.Z --cleanup-tag --yes && git tag -d vX.Y.Z`), fix the problem and tag again.

The `.dmg` is signed with Developer ID and notarized only if the Apple secrets below are set. Otherwise it is only ad-hoc signed and not notarized, and the run shows a warning. Its first launch is then blocked, and users choose *Open Anyway* in *System Settings › Privacy & Security*.

The Linux packages are built on `ubuntu-24.04` rather than `ubuntu-latest`, so they run on any distribution with glibc 2.39 or newer. When GitHub deprecates that image, move `release.yml` and `ci.yml` to the next LTS.

## 3. AUR

The AUR packages build from the published release, so do this after **Publish**.

Once, clone the two AUR repositories:

```sh
git clone ssh://aur@aur.archlinux.org/mdgitjournal.git     ~/aur/mdgitjournal
git clone ssh://aur@aur.archlinux.org/mdgitjournal-bin.git ~/aur/mdgitjournal-bin
```

Each release, on Arch, from this repository's root, fill in the checksums:

```sh
(cd packaging/aur && updpkgsums)
(cd packaging/aur/mdgitjournal-bin && updpkgsums)
git commit -am "packaging: checksums for X.Y.Z" && git push
```

Then publish each package. For `mdgitjournal`:

```sh
cp packaging/aur/PKGBUILD ~/aur/mdgitjournal/
cd ~/aur/mdgitjournal
extra-x86_64-build                    # clean-chroot build, from devtools
namcap PKGBUILD ./*.pkg.tar.zst
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO && git commit -m "Update to X.Y.Z" && git push
```

Repeat for `mdgitjournal-bin`, using `packaging/aur/mdgitjournal-bin/PKGBUILD`. For a packaging-only fix, bump `pkgrel` instead of `pkgver`.

`mdgitjournal-bin` takes the binary from the release `.deb` and everything else from the tagged source. After upgrading Tauri, check the `.deb` still has `usr/bin/mdgitjournal` (`bsdtar -tf`).

## 4. One-time setup: GitHub

**GitHub App client ID.** Commit it in `.cargo/config.toml`, so that release, source and AUR builds all get it. See [docs/github-app.md](docs/github-app.md#2-build-with-the-client-id). Another option is a repository variable, `MDGJ_GITHUB_CLIENT_ID`, but that only reaches the release builds.

**Secrets** for signing and notarizing the `.dmg` (optional; see part 5 for the certificate):

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | The *Developer ID Application* certificate and key exported from Keychain Access as `.p12`, base64-encoded: `base64 -i cert.p12 \| gh secret set APPLE_CERTIFICATE` |
| `APPLE_CERTIFICATE_PASSWORD` | The `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <Team name> (<Team ID>)`, as shown by `security find-identity -v -p codesigning` |
| `APPLE_ID` | Your Apple ID, as a member of the team |
| `APPLE_PASSWORD` | An app-specific password for that Apple ID, from <https://account.apple.com> › *Sign-In and Security* |
| `APPLE_TEAM_ID` | The team's 10-character Team ID |

**Website.** `.github/workflows/pages.yml` deploys `site/` to GitHub Pages on every push to `main` that changes it.

1. *Settings › Pages › Build and deployment › Source*: **GitHub Actions**.
2. At the DNS provider for `stephenfay.xyz`, add a record: `mdgitjournal  CNAME  dcxst.github.io.`
3. *Settings › Pages › Custom domain*: `mdgitjournal.stephenfay.xyz`, then tick **Enforce HTTPS** once the certificate is issued. Workflow deployments ignore `site/CNAME`, so this setting is what counts.
4. Recommended: verify `stephenfay.xyz` at <https://github.com/settings/pages>, so no other account can claim the subdomain.

The privacy policy is then at <https://mdgitjournal.stephenfay.xyz/privacy.html>. Until step 1 is done, every Pages run fails with *Failed to create deployment (status: 404) … Ensure GitHub Pages has been enabled*. The site's *Download* and AUR links point at the first release and the AUR packages, so they lead nowhere until those are published.

## 5. Mac App Store, iOS App Store and TestFlight

Builds are signed by your brother's Apple Developer team. The store lists **his team as the seller and developer**. To move the app to your own account later, use App Store Connect's *App Transfer*. The bundle ID, reviews and TestFlight testers carry over.

The Team ID, certificates and keys stay out of the repository. Keep them in the environment or in GitHub secrets.

### One-time setup

1. **Access.** Your brother, as Account Holder, invites your Apple ID in App Store Connect › *Users and Access*. Pick the **App Manager** role and tick *Access to Certificates, Identifiers & Profiles*.
2. **Team ID.** Find it at <https://developer.apple.com/account> › *Membership details*. Export it in your shell: `export APPLE_TEAM_ID=XXXXXXXXXX`.
3. **App ID.** In *Certificates, Identifiers & Profiles › Identifiers*, click **+**, then *App IDs › App*. Choose an explicit bundle ID, `xyz.stephenfay.mdgitjournal`, and enable no capabilities. One App ID covers iOS and macOS.
4. **Certificates.** Create these in Xcode › *Settings › Accounts › Manage Certificates*, or from a CSR in the portal:
   - **Apple Distribution**: App Store builds for iOS and macOS.
   - **Mac Installer Distribution**: signs the Mac App Store `.pkg`. Keychain Access lists it as *3rd Party Mac Developer Installer*.
   - **Developer ID Application**: the `.dmg` on GitHub Releases, used in the secrets above. Usually only the Account Holder can create this one.
5. **Provisioning profiles.** In *Profiles*, click **+** and create a **Mac App Store Connect** profile and an iOS **App Store Connect** profile. Use the App ID and the Apple Distribution certificate, then download both.
6. **App record.** In App Store Connect › *Apps*, click **+**, then *New App*:
   - Platforms: iOS and macOS. Name: `mdgitjournal`. Bundle ID: `xyz.stephenfay.mdgitjournal`. SKU: `mdgitjournal`.
   - *App Information*: category **Productivity**; privacy policy URL `https://mdgitjournal.stephenfay.xyz/privacy.html`.
   - *App Privacy*: **Data Not Collected**.
   - Age rating: answer **None** throughout. The app has no in-app browser; GitHub opens in the system browser.
   - Support URL: `https://github.com/dcxSt/mdgitjournal/issues`. Marketing URL: `https://mdgitjournal.stephenfay.xyz`.
   - Export compliance: the app uses only standard HTTPS, which is exempt. To skip this question on every build, set `ITSAppUsesNonExemptEncryption` to `false` in `app/src-tauri/Info.plist` and `Info.ios.plist`, which Tauri merges into the bundle.
7. **API key for uploads.** In App Store Connect › *Users and Access › Integrations › App Store Connect API*, create a team key with the App Manager role. Your brother creates it if your role can't. Save `AuthKey_<KEY_ID>.p8` in `~/.appstoreconnect/private_keys/`, then:
   ```sh
   export APPLE_API_KEY=<KEY_ID> APPLE_API_ISSUER=<Issuer ID>
   ```
   Tauri also takes this key as notarization credentials, so the Mac App Store build below unsets these variables.

Every upload needs a new, higher build number, even when the version is unchanged.

### Mac App Store

The store build is sandboxed. [`packaging/macos/mas.entitlements`](packaging/macos/mas.entitlements) grants only the sandbox and outgoing network connections, which git needs. It doesn't need file-access entitlements, because the journal lives inside the app's container: `~/Library/Containers/xyz.stephenfay.mdgitjournal/Data/Library/Application Support/xyz.stephenfay.mdgitjournal/journal`. The `.dmg` build is not sandboxed, so it keeps a separate journal.

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
mkdir -p target/mas
sed "s/TEAM_ID/$APPLE_TEAM_ID/g" packaging/macos/mas.entitlements > target/mas/entitlements.plist
cp ~/Downloads/<Mac App Store profile>.provisionprofile target/mas/embedded.provisionprofile
cat > target/mas/tauri.conf.json <<EOF
{
  "bundle": {
    "macOS": {
      "signingIdentity": "Apple Distribution: <Team name> ($APPLE_TEAM_ID)",
      "entitlements": "$PWD/target/mas/entitlements.plist",
      "files": { "embedded.provisionprofile": "$PWD/target/mas/embedded.provisionprofile" },
      "bundleVersion": "<build number>"
    }
  }
}
EOF

# Store builds are reviewed, not notarized. Unset every variable Tauri reads for notarization;
# otherwise it finds the API key from step 7 and sends this build to Apple's notary service.
env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH \
  pnpm -C app tauri build --bundles app --target universal-apple-darwin --config "$PWD/target/mas/tauri.conf.json"

xcrun productbuild --sign "3rd Party Mac Developer Installer: <Team name> ($APPLE_TEAM_ID)" \
  --component target/universal-apple-darwin/release/bundle/macos/mdgitjournal.app /Applications \
  target/mas/mdgitjournal.pkg
xcrun altool --upload-app --type macos --file target/mas/mdgitjournal.pkg \
  --apiKey "$APPLE_API_KEY" --apiIssuer "$APPLE_API_ISSUER"
```

Instead of `altool`, you can drag the `.pkg` into Apple's **Transporter** app. `notarytool` is only for the `.dmg`, and the release workflow runs it.

### iOS

This needs full Xcode with its licence accepted (`sudo xcodebuild -license accept`), and you signed in to Xcode with your Apple ID (*Settings › Accounts*).

1. Run `pnpm -C app tauri ios init` once, and commit `app/src-tauri/gen/apple`. It is generated from `app/src-tauri/project.ios.yml` (see the README's iOS section), which links libz and libiconv for libgit2.
2. Install the iOS icons (see [Icons](#icons)).
3. Check that `Info.ios.plist` sets `UIFileSharingEnabled` and `LSSupportsOpeningDocumentsInPlace` to `true`, so the journal shows up in the Files app.
4. Build and upload:

```sh
rustup target add aarch64-apple-ios
export APPLE_DEVELOPMENT_TEAM=$APPLE_TEAM_ID
pnpm -C app tauri ios build --export-method app-store-connect --build-number <build number>
xcrun altool --upload-app --type ios --file app/src-tauri/gen/apple/build/arm64/mdgitjournal.ipa \
  --apiKey "$APPLE_API_KEY" --apiIssuer "$APPLE_API_ISSUER"
```

Or drag the `.ipa` into Transporter.

### TestFlight and review

1. After processing, which takes about 15 minutes, each build appears under **TestFlight**. Add yourself as an internal tester and install it with the TestFlight app; macOS builds work the same way.
2. To submit, fill in the version page and choose the build:
   - Screenshots: 6.9″ iPhone, and Mac at 2880 × 1800.
   - Description, keywords.
   - Review notes: *No account is needed; the journal works offline. GitHub sync is optional, under Settings › Connect GitHub.*
3. Submit for review.

## Icons

`assets/icon.svg` is the master, with macOS proportions: a rounded square with margin and shadow. `assets/icon-ios.svg` is the same artwork full-bleed and opaque, because iOS draws its own corners. After editing either, regenerate the icons. Write to a temporary directory first, so the unused Windows and Android sizes stay out of the repository:

```sh
pnpm -C app tauri icon ../assets/icon.svg -o /tmp/mdgj-icons
cp /tmp/mdgj-icons/{32x32,128x128,128x128@2x,icon}.png /tmp/mdgj-icons/icon.icns app/src-tauri/icons/

pnpm -C app tauri icon ../assets/icon-ios.svg --ios-color '#fbfaf7' -o /tmp/mdgj-ios-icons
cp /tmp/mdgj-ios-icons/ios/*.png app/src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset/
```

If App Store Connect rejects the iOS icon for having an alpha channel (ITMS-90717), re-export `AppIcon-512@2x.png` without alpha, for example from Preview with *Alpha* unticked.
