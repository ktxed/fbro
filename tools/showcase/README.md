# fbro macOS showcase

Ten repeatable chapters drive the **real native fbro app** with a visible mouse
pointer. Each chapter gets a fresh starting state, so a retake does not depend
on earlier clips. Recordings are silent 1920×1080 H.264 MP4s at 30 fps, without
captions or click highlights. Most chapters take 15–40 seconds; the extended
Markdown and Git chapters take longer. Loading and action times determine the
actual length.

The opening chapter starts already in grid view with the entire configured
photos folder, without filtering or reducing it to six copied images.
Cmd+3 opens quick preview on the right; clicking through three thumbnails shows
each photo larger without leaving the grid. The first three JPEG/PNG entries
in the listing are used for preview. It then closes preview, opens the
command palette and About fbro, and zooms from the default 13px through three Cmd+Plus presses to 16px
while About is visible. Every later chapter starts at 16px, including its viewer, comparison,
and embedded editor, so cuts keep a consistent zoom level. Each isolated app
session is seeded with this setting before recording starts.

## Setup

Run from the repository root, on a logged-in macOS desktop:

```sh
brew install cliclick ffmpeg midnight-commander
npm ci
npm run showcase -- preflight
```

The runner uses the installed Swift compiler to build its small macOS helper
under ignored `artifacts/showcase/bin/`. It uses the existing `ws` dependency;
there are no additional npm dependencies or changes to fbro's protocol.

In **System Settings → Privacy & Security**, enable **Accessibility** and
**Screen & System Audio Recording** for the app launching the command
(Terminal, iTerm, or Codex). Quit and reopen that app after granting access.
When necessary, add the launching app with the `+` control in the relevant
settings pane. Terminal runs and Codex runs can have different permission
status. Preflight reports the actual status without prompting, recording, or
sending input, and saves `artifacts/showcase/preflight.json`.

The supplied [config.json](config.json) selects `/Applications/fbro.app`,
`/Users/ktx/Documents/pics`, six JPEGs, a GIF, a MOV, and `spex/sources.md`.
Adjust filenames there to choose the media that should appear in the footage.
The source root is discovered from the tooling's location. On this Mac,
`/usr/bin/nano` launches Pico; the Quick Edit scene uses bindings common to
Nano and Pico. A GNU Nano installed earlier on PATH works as well.

Use a current fbro build containing these features. To build and select the
checkout's release binary:

```sh
npm run tauri -- build --no-bundle
npm run showcase -- preflight --app src-tauri/target/release/fbro
```

Preflight checks tools, app availability, display geometry, permissions, and
selected media metadata. Native image/diagram rendering and media playback
are checked during the capture check and rehearsals.

## Rehearse and record

```sh
npm run showcase -- list
npm run showcase -- storyboard
npm run showcase -- preflight --capture
npm run showcase -- rehearse --clip 01
npm run showcase -- rehearse --clip 05,06,09,10
npm run showcase -- record --clip all
npm run showcase -- record --clip 09,10
```

Both two-digit IDs and full chapter IDs are accepted. `--config FILE`,
`--app BINARY`, and `--pace NUMBER` can override the defaults. Pacing defaults
to `1`; `1.5` increases the intentional pauses by 50%.

**Rehearsal performs the real demo operations**, including the local demo
commit in chapter 10; it only skips recording. `storyboard` and `list` describe
the chapters without operating the app or creating demo fixtures.

Keep fbro in the foreground while a run is active. The runner positions a
fixed stage on the chosen display, preserves it throughout a take, and
fits secondary viewer/compare windows inside it. Every chapter uses the same
display working area so native maximize remains fully visible and zoom stays
consistent between clips;
the video adds padding to preserve proportions in its 16:9 output.
Its capture crop is converted
from logical display coordinates to physical pixels for Retina displays.
Use Focus mode to keep notifications out of the footage.

Press **Ctrl+C** in the launching terminal to interrupt a run. The runner
stops input, finalizes the current recording, closes its own fbro process,
and writes the failed-take index. It does not retry a delete or commit.
Only the dedicated fbro PID is controlled; an existing fbro instance is left
running. App settings, paths, and session files use an isolated configuration
directory and a separate loopback WebSocket port.

## Chapters

| ID | Feature and scene |
| --- | --- |
| `01-browse-filter` | Entire photos folder in grid immediately → Cmd+3 quick preview on the right → select three photos → close preview → command palette → About → zoom → themes → list → filter → clear |
| `02-collect-shortlist` | Create Shortlist → select six originals → grid → copy → navigate into copied thumbnails → list |
| `03-remove-rejects` | Delete one copied photo in grid → list → select/delete two together → remaining thumbnails |
| `04-rename-keeper` | Choose cover photo → grid → rename to cover.jpg → list → rename Shortlist to Presentation → preview its grid |
| `05-markdown-mermaid` | Open sources.md → maximize → search raw Markdown → next/previous occurrences → keyboard scroll down/up/right/left → Markdown preview → expanded Mermaid diagram and zoom → restore |
| `06-media-viewers` | JPEG zoom → next image → animated GIF → play/pause MOV |
| `07-compare-notes` | Select two note versions → compare → maximize briefly → close → search for notes → To Results → select a result → Info (`i`) → close |
| `08-explore-git` | Real checkout: Git status → log → expand commit → historical diff → maximize; navigate/switch views only for multiple changes |
| `09-edit-review` | Independent clone: edit README → review diff → terminal → ls -al → mc → maximize → promote to tab → return to files |
| `10-stage-commit` | Independent clone: stage README → message → commit → new history entry → maximize diff briefly |

Chapter 08 inspects the checkout's current branch, history, and pending tooling
changes. It performs no staging, branch switching, fetching, or committing.
The Git modal's remote-operation controls are not part of these scenes.

Comparison scenes read the visible change-block counter. With zero or one
block they only maximize, pause briefly, and close. With multiple blocks they
also navigate down/up, show unified mode, and return to side-by-side mode.
Chapter 07 then searches its demo folder for `shortlist-notes-*.txt`, mounts
the results in the file panel, and opens Info with the registered `i` shortcut
on `shortlist-notes-v2.txt`. The modal shows the real file’s path and metadata;
closing it returns to that search result. These actions are included in the
clip’s scene and keyboard metadata for overlays.
The demo README paragraph is inserted before `## Design docs`, around the
middle of the document, leaving surrounding content visible on both sides.

After editing and reviewing README, chapter 09 runs `ls -al` and `mc` in a real
shell rooted in the independent demo repository. It maximizes the embedded
terminal, promotes the same live PTY session to a sibling tab, and clicks back
to the original files tab before continuing.
The terminal remains open in its tab until the chapter's dedicated app exits.
Preflight checks that Midnight Commander (`mc`) is installed.

## Demo data and editing files

Photo operations create copies under:

```text
/Users/ktx/Documents/pics/fbro-demo/<run>/<chapter>/
```

Git chapters use fresh, independent clones under:

```text
<fbro>/artifacts/showcase/<run>/<chapter>/fbro-demo/
```

Clones preserve the committed history at the source revision recorded in the
manifest. They use their own Git metadata and object store, with no remotes,
a `demo/showcase` branch, a local demo author, and locally disabled signing
and hooks. No worktree, shared index, or object-store alternate is used.
Chapter 10 verifies that only README is modified and staged before clicking
Commit, then checks the new commit's parent, message, contents, and clean status.

The committed sentence is:

> Showcase: browse, organize, and review files in fbro.

The message is `docs(showcase): describe the demo workflow`. These synthetic
local commits are demo outputs. The tooling branch is never committed, merged,
or pushed by the runner.

Each run saves the following under `artifacts/showcase/<run>/`:

Run names are random playful pairs such as `happy-parakeet` or
`dancing-waffle`, including the folder names visible in fbro. Existing names
are skipped so retakes always get fresh destinations. Recording dates remain
in the metadata and logs.

- Numbered `.mp4` clips, or no video for rehearsal.
- `clips.md`: chapter features, filenames, status, durations, action timestamps,
  failures, and demo commit hashes, ready to use as an editing index.
- `manifest.json`: machine-readable versions of that information, asset hashes,
  source revision, app version, configuration, stage geometry, and crop.
- Per-chapter logs: app output, backend messages, dispatched commands and keys
  (`input.jsonl`), FFmpeg output, and an
  Accessibility snapshot on failure when the process remains available.

Setup is performed before recording. The runner verifies the original
checkout's HEAD, branch, refs, index, tracked diff, and untracked status, plus
the hashes of the selected original media, after every run. Make unrelated
edits outside the recording session so this check can detect unexpected changes.
Demo folders and clones are retained; remove unwanted runs manually after
editing. A retake never resets or deletes an existing demo repository.

## Join and speed up recorded clips

```sh
npm run showcase -- montage
npm run showcase -- montage --speed 3
npm run showcase -- montage --input artifacts/showcase/final-happy-puffin --speed 5 --output artifacts/showcase/highlights.mp4
```

The script also runs directly:

```sh
node tools/showcase/montage.mjs --speed 5 --output artifacts/showcase/highlights.mp4
```

`montage` joins the clips in manifest order and defaults to 5× speed. Without
`--input`, it uses the folder named in `artifacts/showcase/latest-complete.txt`.
An input must contain only passed recorded clips. The result defaults to
`showcase-5x.mp4` inside that folder, with a JSON report and Markdown editing
index beside it. Chapter markers and action timings use the accelerated
timeline. Existing outputs are never overwritten; choose another output name
for a retake. Originals are preserved.

The output is silent H.264 MP4 at 30 fps, preserving the recording dimensions.
This command only needs Node, FFmpeg, and FFprobe; it does not open fbro or
require macOS capture permissions.

## Overlay data for animation

[OVERLAY_FORMAT.md](OVERLAY_FORMAT.md) documents the current montage report
and the version-1 overlay timeline contract: scene descriptions, operation
captions, and keyboard badges synchronized to output frames. It includes
timing conversion, repeated-key grouping, provenance, and a complete
[example JSON file](overlay.example.json).

The overlay timeline is specified but not yet exported. The existing montage
JSON already contains accelerated chapter/action start times; raw keyboard
logs still need an explicit recording-relative clock for precise export.

## Accessibility diagnostics and calibration

UI targets normally use Accessibility labels and bounds. Commands use the
backend's registry bindings; actions are checked against backend snapshots and
patches. Pointer movement uses cliclick. The native helper uses physical
CGEvent keys for shortcut chords and Unicode events for text, preserving
Cmd/Ctrl/Option modifiers independently of typing. This also prevents shifted
characters from triggering listing shortcuts while entering a filter.

If a particular WebKit build does not expose a control, the failure log contains
its Accessibility snapshot. Increase `timeoutMs` while calibrating, and inspect
the dedicated PID printed by the runner from a second terminal:

```sh
npm run showcase -- inspect --pid 12345
npm run showcase -- calibrate --pid 12345 --name command.file.compare
```

For calibration, place the pointer at the desired control before invoking the
command without moving the pointer. Copy the printed entry into `anchors` in
the configuration. Anchors are a fallback for command-palette results
(`command.<command-id>`) and named file rows (`entry.<filename>`). They include
the exact stage dimensions and are rejected when those dimensions change.
Other controls require their existing Accessibility labels; the runner stops
on missing targets rather than guessing a click.

## Check the recording setup

```sh
npm run showcase -- preflight --capture
npm run showcase -- rehearse --clip all
```

All scenes perform real fbro actions on real files. Inspect the capture-check
MP4 for cursor visibility, readability, cropping, and inclusion of the viewer.
Rehearsal lets you check the pacing and control locations before recording.
FFprobe checks each completed recording's codec, dimensions, frame rate,
duration, and lack of audio.

The native recording implementation is macOS-only. Linux and Windows are not
native capture targets for this tool; `list` and `storyboard` remain available
there.

## Tool choices

Existing options considered were [cliclick](https://github.com/BlueM/cliclick)
with [FFmpeg](https://ffmpeg.org/ffmpeg-devices.html#avfoundation),
[Hammerspoon](https://www.hammerspoon.org/) with
[OBS](https://obsproject.com/kb/macos-screen-capture-source), and
[Tauri's embedded WebDriver route](https://v2.tauri.app/develop/tests/webdriver/).
The chosen tools provide real pointer movement and controllable recordings
without adding an application plugin or two GUI apps. The small runner supplies
fbro-specific scene preparation, native target lookup, synchronization, and the
editing index; it does not add a new application automation protocol.
