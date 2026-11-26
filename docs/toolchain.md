# Toolchain pinning (mise + rust-toolchain.toml)

This page covers how fbro pins the versions of Node, Rust and its dev tools,
why it is set up this way, and how to change a version.

## Why

Before this setup, no version was pinned anywhere:

| Where | Node | Rust | Other |
|---|---|---|---|
| CI (`ci.yml`, `pages.yml`, `release.yml`) | `setup-node` with `lts/*` | `dtolnay/rust-toolchain@stable` | gitleaks 8.30.1, downloaded with curl in `ci.yml` |
| e2e Docker (`docker/e2e/Dockerfile`) | `node:24` | `rust:1` | — |
| README / site install page | "22.19 or newer" | "current stable" | `cargo-audit` "if installed" (TESTING.md) |

A new Node LTS or Rust stable release could change or break CI even though
nothing in the repo changed. CI could also differ from developer machines.
Now each version is set in exactly one checked-in file.

## Where each version lives

| Tool | Pinned in | Installed by |
|---|---|---|
| Rust (rustc, cargo, rustfmt, clippy) | `rust-toolchain.toml` | rustup |
| Node | `mise.toml` (+ `mise.lock`) | mise |
| gitleaks | `mise.toml` | mise (aqua registry) |
| cargo-audit | `mise.toml` as `cargo:cargo-audit` | mise, which builds it with cargo |
| mise itself (CI only) | `version:` input of `jdx/mise-action` in each workflow | the action |

**Change versions in these files, never in workflow files.** Use
`mise up --bump` for the mise tools. For Rust, edit `channel` in
`rust-toolchain.toml`.

`package.json` `engines.node` (`>=22.19.0`) is the minimum Node version that
can build fbro. It is not the pinned version, and the two are separate on
purpose.

### Why Rust is not in mise.toml

- rustup reads `rust-toolchain.toml` on its own, from the current directory and
  every parent directory. That covers contributors without mise, IDEs and
  rust-analyzer, tauri-action, and `tools/lfs-spy`, because the file sits at
  the repo root.
- mise's `rust` backend only calls rustup. Adding `rust` to `mise.toml` as well
  would export `RUSTUP_TOOLCHAIN`, which overrides `rust-toolchain.toml`, and
  there would then be two pins to keep in sync.
- rustup 1.28 and later no longer installs a missing pinned toolchain on first
  use. Run `rustup toolchain install` (no arguments) once; it installs the
  toolchain named in the file. CI does the same.

### cargo-audit

cargo-audit is not in the mise registry, so it comes from the `cargo:`
backend. mise builds it from source, or uses cargo-binstall when that is
installed. Only the local `audit` task uses it, so no CI job installs it.

## Tasks

`mise.toml` defines tasks that wrap the existing npm scripts; it does not
replace them. CI runs these same tasks, so a CI failure can be reproduced
locally with one command:

| Task | Runs | Used by |
|---|---|---|
| `mise run ci:test` | `npm ci`, version checks, `npm run build`, `cargo test` | `ci.yml` `test` |
| `mise run ci:site` | `npm ci` and `npm run build` in `site/` | `ci.yml` `site`, `pages.yml` |
| `mise run secrets` | `gitleaks detect` over the git history | `ci.yml` `gitleaks` |
| `mise run audit` | `cargo audit` on both Rust lockfiles | local only |

The e2e smoke command is not a task. It is run by hand (see `TESTING.md`).

## Local setup

```sh
# install mise: https://mise.jdx.dev/getting-started.html
mise install            # Node, gitleaks, cargo-audit
rustup toolchain install
mise ls                 # check the active versions
rustup show active-toolchain   # should say "overridden by …/rust-toolchain.toml"
```

If nvm is also installed, activate mise after nvm in your shell rc, so that
`node -v` reports the pinned version inside the repo.

## CI (GitHub Actions)

The setup follows <https://mise.jdx.dev/continuous-integration.html#github-actions>.

- Every job uses `jdx/mise-action@v5` with `version:` pinned, so mise itself
  only updates when that line is edited. `install_args` lists only the tools
  each job needs (`node` or `gitleaks`).
- Once a `mise.lock` exists, the action adds `--locked` to `mise install` on
  its own. CI then installs exactly the locked, checksummed artifacts.
- The action caches the installed tools. mise's docs warn that a restored tool
  cache is not re-checked against `mise.lock`. The strict alternative is
  `cache: false` plus `actions/cache` on the `downloads-cas` store, which is
  slower. The default cache is acceptable for this repo.
- The action does not cache npm, which `setup-node` used to do. `ci.yml` and
  `pages.yml` therefore cache `~/.npm` with `actions/cache`, keyed on the
  relevant `package-lock.json`. `release.yml` has no npm cache: the cache path
  differs on Windows and macOS, and releases are rare.
- Rust jobs run `rustup toolchain install`, which replaces
  `dtolnay/rust-toolchain@stable`. The macOS release adds `aarch64-apple-darwin`
  with `rustup target add`.
- tauri-action uses the node and cargo found on `PATH`, and mise-action adds
  its shims to `PATH`.
- `pages.yml` also runs when `mise.toml` or `mise.lock` changes, so a Node
  bump rebuilds the guide.
- The action gets the workflow `github_token` by default, which avoids GitHub
  API rate limits when it installs tools. CI jobs hold no secrets, so
  `MISE_SAFE` is not needed for fork PRs.

## Lockfile

Generate `mise.lock` for every platform CI builds on, and commit it:

```sh
mise lock --platform linux-x64,macos-arm64,windows-x64
```

`mise install --locked` fails for any tool that has no entry in the lockfile.
Regenerate the lockfile after every `mise up --bump`.

## Not covered

- The e2e Docker base images (`node:24`, `rust:1`) are still unpinned. This was
  a deliberate choice.
- Linux system packages (webkit2gtk, the ffmpeg dev libs, GStreamer) still come
  from `apt-get` in the workflows. mise does not manage distro packages.
- The macOS and Windows release builds only run on a tag push. They are
  unverified until the next release.
