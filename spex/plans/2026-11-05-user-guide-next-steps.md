# User guide: next steps

**Goal:** finish the user guide at https://ktxed.github.io/fbro/ (`site/`):
screenshots on every page where a picture helps, a few polish items, and the
first deployment.

**Where things stand (2026-11-05, branch `docs/user-guide-site`):**

- Checkpoint commit `40aed93`: all guide sections (Get started, Using fbro, File
  operations, Viewing & editing, Bros, Git, Remote fbro, Customize, Reference,
  Help), the per-platform shortcut snapshot, and the Pages workflow.
- Uncommitted, reviewed and accepted: the screenshot runner
  (`npm run docs:screens`, `tests/e2e/docs/`), the `<Screenshot>` component,
  and five scenes (`first-launch`, `select-files`, `grid-view`, `copy-dialog`,
  `viewer-markdown`). Documented in `TESTING.md` and `AGENTS.md`.
- Bugs and spec drift found while writing are in
  [known-issues.md](known-issues.md).

**Decisions already made:** dark scheme only; no video clips for now; Linux
captures (F-key hints) are fine.

**Constraints:** keep working on `docs/user-guide-site` and squash it into
`development` at the end. No commits without user validation. Every scene
must run on its own (`npm run docs:screens -- <name>`). Captures show fbro as
it draws itself; the only thing the runner hides is the Docker container's
bind mounts in the source bar.

---

## Phase 1: commit the screenshot runner

1. Commit the runner, component, five scenes and docs as one checkpoint
   (`docs(site): capture guide screenshots from the real app`).

## Phase 2: remaining screenshots

About 20 scenes, grouped by what they need. Each row is one scene, named after
the image file it writes.

### 2a. Same fixture, single instance

These only need the current fixture and the patterns already in
`tests/e2e/docs/scenes.mjs`.

| Scene | Page | How |
|---|---|---|
| `filter` | using/sorting-filtering | Website folder, `f`, type `md`; filter field open and footer shows `/md/` |
| `sort-selector` | using/sorting-filtering | Shift+S; selector open on Date, descending |
| `bookmarks` | using/bookmarks-history | Bookmark two demo folders via the popover, then open it on a third |
| `command-palette` | using/command-palette | Ctrl+Shift+P, type `grid` |
| `cheat-sheet` | using/command-palette | `?` |
| `context-menu` | using/command-palette | `c` on a selection of two files |
| `inline-choices` | files/inline-choices | Cursor on a zip archive (add one to the fixture): Browse archive first |
| `bro-list` | bros/overview | F1 on two selected folders; zip/7z/tar ranked first |
| `quick-preview` | viewing/quick-preview | Photos in list view, Ctrl+F3, cursor on a photo |
| `terminal` | viewing/terminal | `t` in the website folder, run `ls -la` (shell runs in the container) |
| `settings-general` | customize/settings | Ctrl+, on General |
| `settings-shortcuts` | using/command-palette | Settings → Shortcuts, search `copy` |
| `color-scheme-editor` | customize/color-schemes | Display → Customize…, Duplicate fbro Dark as "Midnight" |

### 2b. Need extra fixture content

Add to `tests/e2e/docs/fixture.mjs`, keeping fixed timestamps:

- **Git repository:** `demo/garden-app` with three commits made with fixed
  `GIT_AUTHOR_DATE` / `GIT_COMMITTER_DATE` and a neutral author
  ("Lighthouse volunteer <volunteer@example.org>"), then one modified, one
  staged and one untracked file. The backend runs `git` inside the container;
  `tests/e2e/smoke/cases/git.mjs` shows it is available.
- **Duplicates:** copies of two photos under `demo/backup/website-2026-10/assets`
  so Fedup finds groups.
- **Conflict:** `demo/backup/README.md` already exists (it does), so copying the
  website's README with **Ask** produces a conflict prompt.
- **A large file** (about 6 GB, sparse) for a job that runs long enough to
  capture. Create it with `truncate`, not by writing bytes.

| Scene | Page | How |
|---|---|---|
| `git-listing` | git/repositories | Repository folder: status markers and branch in the path bar |
| `git-changes` | git/repositories | Shift+G, Changes tab with a commit message typed |
| `git-log` | git/repositories | Log tab, first commit expanded |
| `conflict-prompt` | files/operations | Copy README.md to backup with Ask; capture the "File already exists:" prompt |
| `jobs-dialog` | files/jobs | Start the large-file copy, send it to the background, `j`; capture while running, then cancel |
| `fedup-results` | bros/duplicates | Global selection over `demo`, Fedup Bro, results view |
| `hash-results` | bros/hashes | Generate hashes on the photos, results view |

### 2c. Separate windows

The viewer already works by loading the viewer page into the main WebView
(see the `viewer-markdown` scene). Do the same for the diff window:

| Scene | Page | How |
|---|---|---|
| `diff-window` | viewing/compare | Compare `CHANGELOG.md` with a modified copy; load the diff page the way `tests/e2e/smoke/cases/diff.mjs` does |
| `editor-tab` | viewing/editor | Open `notes.txt` in an editor tab with quick preview on in the other panel (Markdown note for a rendered preview) |

### 2d. Two instances (remote fbro)

Start the second browser (`fbro-b`) as `tests/e2e/smoke/index.mjs` does, and
reuse the pairing steps from `tests/e2e/smoke/cases/remote.mjs`.

| Scene | Page | How |
|---|---|---|
| `pairing` | remote/pairing | Both sides show "Confirm pairing with …"; capture instance A |
| `sharing-settings` | remote/pairing | Settings → Sharing on A after pairing, with a shared folder added |
| `remote-browse` | remote/browse-transfer | A's left panel on B's shared photos in grid view (remote thumbnails) |
| `sync-results` | remote/sync | Sync Bro between a local copy and B's folder with one file changed |

The pairing code is random, so it differs on every run. That's acceptable; the
fingerprint in Sharing settings also differs.

### 2e. Annotated layout (First launch)

Add numbered callouts over `first-launch` (tab bar, path bar, source bar,
listing, footer, button bar) as an SVG overlay in the page, not baked into the
image. Positions are percentages of the image, so they survive re-captures as
long as the layout doesn't move.

### Acceptance

- `npm run docs:screens` runs all scenes in one go without failures.
- Every new image is used by a page and has meaningful alt text.
- The site builds and the link/anchor check passes.
- Total size of `site/src/assets/screens/` stays under about 3 MB.

## Phase 3: polish

1. **Home page cards** link to their sections (Bros → bros/overview, Git →
   git/repositories, Remote → remote/pairing, and so on).
2. **Home hero image:** keep the macOS screenshot for now (decided
   2026-11-05). Revisit if the home page gets a new layout.
3. **Site build in CI:** add a job to `.github/workflows/ci.yml` that runs
   `npm ci && npm run build` in `site/`. It's Node-only and fast, and catches
   unknown `<Shortcut id>`s and missing screenshots on every pull request.
4. **Help link in the app:** add a **User guide** action to the About dialog
   (and optionally a palette command) that opens the site in the browser. This
   is an app change: it uses the existing `open_url` platform method, follows
   `spex/modal-stack.md` for the dialog, gets a smoke case, and should be its
   own commit (`feat(ui): …`).
5. **Edit links:** confirm Starlight's "Edit page" links point to
   `development` once the branch is merged.

## Phase 4: ship

1. User review of the whole site (`npm run site:dev`), page by page, ideally
   next to a running fbro.
2. Squash-merge `docs/user-guide-site` into `development`.
3. In the repository's **Settings → Pages**, set **Source** to **GitHub
   Actions**. If the `github-pages` environment only allows the default
   branch, add `development` to its deployment branches.
4. Check that the Pages workflow runs, and that the deployed site loads
   correctly under `/fbro/` (styles, search, images).
5. Mention the guide in the next release notes.

## Later

- Fix the items in [known-issues.md](known-issues.md), each on its own
  branch, updating the guide pages that mention them (notably New File
  truncation and the "Delete is permanent" caution if trash support lands).
- Native macOS and Windows checks of the guide's platform-specific
  statements (install steps, shortcuts, notifications).
- Optional later: short clips from `tools/showcase` for the home page and
  tour, light-scheme screenshots, translations (Starlight supports i18n).
