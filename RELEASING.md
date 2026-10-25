# Releasing

A release is a version tag pushed from `development`. GitHub Actions
(`.github/workflows/release.yml`) builds the installers and attaches them to a
draft GitHub Release named `fbro <tag>`.

## 1. Update `development`

Fetch and rebase onto `origin/development` before tagging. The workflow builds
the tagged commit. A stale local branch leaves commits that are already on the
remote out of the release.

## 2. Set the version

Use one version string, with no `v` prefix, in all of these:

- `package.json`
- `package-lock.json` (the `fbro` package only)
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml` (the `[package]` version)
- `src-tauri/Cargo.lock` (the `fbro` package)

Leave `tools/lfs-spy` on its own version. A prerelease uses the same string
everywhere, for example `0.0.X-rc.1`.

## 3. Check

```bash
node scripts/check-version.mjs --tag 0.0.X
```

The command exits 0 when `package.json`, `tauri.conf.json`, and `Cargo.toml`
agree and the tag equals the `tauri.conf.json` version. A `v` prefix fails.
The script does not read the lockfiles; keep them in sync anyway.

The release workflow runs this check before it compiles. A mismatch fails
Ubuntu, macOS, and Windows together.

## 4. Commit

Commit the version bump on `development`, for example
`chore(release): bump version to 0.0.X`. The tag has to point at that commit.
Tagging an older commit that still has the previous version fails the check.

## 5. Tag and push

Create a lightweight tag whose name is the version string, then push the
branch and the tag:

```bash
git tag 0.0.X
git push origin development
git push origin 0.0.X
```

Pushing the tag starts the workflow. Pushing a tag name that already exists
does not start another run.

To move a tag onto a new commit, point it at that commit and force-update the
remote tag. That push starts a new run.

```bash
git tag -f 0.0.X
git push origin development
git push origin +refs/tags/0.0.X
```

Deleting the remote tag (`git push origin :refs/tags/0.0.X`) and pushing it
again does the same thing.

## What gets published

The workflow runs on `ubuntu-24.04`, `macos-latest` with
`--target aarch64-apple-darwin`, and `windows-latest`. One platform failing
does not cancel the others. The Linux packages need the glibc and FFmpeg
from Ubuntu 24.04, so they do not start on Ubuntu 22.04.

Assets land on the draft release, not as Actions artifacts:

- Linux: AppImage, deb, and rpm. The AppImage bundles the Ubuntu 24.04
  GStreamer plugins installed by the workflow, and WebKit plays video with
  that set alone. `gstreamer1.0-plugins-bad` and `gstreamer1.0-libav` supply
  the H.264 parser, decoder, and WebVTT encoder.
- Windows: NSIS installer and MSI. These builds are unsigned, so SmartScreen
  may warn until you choose More info, then Run anyway.
- macOS: Apple silicon disk image. The app bundle is ad hoc signed and not notarized.
  The first time, right-click the app and choose Open.

macOS uses `bundle.macOS.signingIdentity: "-"` to bind the complete app bundle
and Info.plist to `dev.fbro.app`. This is required for native notification
authorization; the compiler/linker's executable-only signature is insufficient.
Ad hoc signing needs no Apple Developer account and does not establish a
Developer ID or remove Gatekeeper warnings. An Apple signing identity can be
provided for certificate-signed releases later.

There is no Intel Mac build and no in-app updater. The workflow always creates
a draft and does not mark the release as a prerelease, including for tags such
as `0.0.X-rc.1`. Tag names must match `[0-9]+.[0-9]+.[0-9]+` or
`[0-9]+.[0-9]+.[0-9]+-*`.

## Linux-only build

Actions → Release → Run workflow, on `development`, with "Build only Linux"
left checked. That run checks that the version files agree and uploads the
AppImage, deb, and rpm as workflow artifacts. It does not publish a GitHub
Release and does not build Windows or macOS.

```bash
gh workflow run release.yml --ref development -f linux_only=true
```
