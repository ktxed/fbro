# Showcase overlay timeline, version 1

This document defines the sidecar data for animations placed over a showcase
montage, including scene descriptions, operation captions, and keyboard badges.
It is an editor-independent JSON contract for the Effectscraft overlay work;
it does not assume an Effectscraft import API.

**Implementation status:** the three-track format below is specified but is
not yet exported by the recording or montage scripts. The existing montage
report has a different shape, documented at the end of this file. Do not pass
that report to a consumer expecting this format.

Use `<video-name>.overlays.json` beside the final video. See the complete,
illustrative [overlay.example.json](overlay.example.json); its timings are
examples, not annotations for the current recording.

## Envelope

| Field | Type | Meaning |
| --- | --- | --- |
| `format` | string | Exactly `fbro.showcase.overlays`. |
| `version` | integer | Exactly `1`. Reject unsupported versions. |
| `video.file` | string | Video path relative to the sidecar directory. |
| `video.width`, `video.height` | positive integers | Final encoded pixel dimensions, including any padding. |
| `video.fps` | positive integer | Constant output frame rate; current montages use `30`. |
| `video.frameCount` | positive integer | Actual encoded video frame count. |
| `video.speed` | positive number | Constant playback acceleration already applied to every clip, e.g. `5`. |
| `clips` | array | Source clips in montage order. |
| `tracks.scenes` | array | Screen/context descriptions. |
| `tracks.operations` | array | Descriptions of what the user is doing. |
| `tracks.keyboard` | array | Key chords and grouped repeated presses. |

All fields in this document are required unless marked optional. Empty tracks
are represented by `[]`. IDs are nonempty strings and are unique across clips
and all events. Consumers may ignore additional fields within version 1.

Each `clips` item contains:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | Stable source chapter ID, e.g. `01-browse-filter`. |
| `file` | string | Original clip path relative to the sidecar directory. |
| `sourceDurationSeconds` | positive number | Probed duration at original recording speed. |
| `startFrame`, `endFrame` | integers | The clip's interval in the final montage. |

Paths are data, not commands. Consumers must not execute them. Moving a
sidecar/video bundle requires preserving or updating its relative paths.

## Time and synchronization

The only rendering clock is the final video's frame index. Frame zero is the
first decoded frame. All intervals are **start inclusive, end exclusive**:
an event is visible when `startFrame <= frame < endFrame`.

- Frames are integers with `0 <= startFrame < endFrame <= video.frameCount`.
- Events must fit within their referenced clip. Scene and operation references
  must belong to that same clip.
- Tracks are sorted by `startFrame`, then `id`. An array's order is not a
  layer/z-index instruction.
- Scenes cannot overlap. Operations within a scene cannot overlap. Keyboard
  badges cannot overlap other keyboard badges. Different tracks can overlap.
- Gaps are allowed: an empty interval renders no overlay on that track.
- A keyboard event must fit within its operation, and an operation within its
  scene. If context changes mid-operation, split the operation into two events.
- Do not apply `video.speed` again when rendering: every frame value is already
  on the accelerated timeline.

For an untrimmed, straight concatenation at constant speed, map a source time
to a frame using:

```text
montageSeconds = (sum(previous source clip durations) + sourceSeconds) / speed
frame = floor(montageSeconds * fps + 0.5)
```

Use unrounded source durations for the sum; do not accumulate rounded frame
lengths. Adjacent clips share a boundary. The final clip ends at the actual
encoded `frameCount`, which can include a few extra frames from constant-frame-
rate encoding. Clamp point events to their clip's last frame. If a very short
interval rounds to zero frames, give it one frame within its parent interval.

The frame intervals are authoritative after manual trimming, reordering, or
retiming. Rebuild them after changing the edit; the formula above only applies
to the current constant-speed concatenation.

## Shared event fields and provenance

Every event has `id`, `clipId`, `startFrame`, `endFrame`, and `source`.
`source` describes the original observation separately from the overlay's
display interval:

| Field | Type | Meaning |
| --- | --- | --- |
| `source.startSeconds` | number | Original clip-relative time; zero or greater. |
| `source.endSeconds` | number, optional | Observed end, at or after the start and within the original clip. Omit if unknown. |
| `source.timing` | string | `recorded`, `estimated`, or `authored`. |

`recorded` means synchronized recording-relative observations. `estimated`
means inferred alignment, such as wall-clock logs aligned manually to footage.
`authored` means an editorial annotation without a measured trigger. Never
present inferred timing as recorded timing.

Display intervals may be extended for readability without changing the source
observation. A caption's `endFrame` therefore does not establish that a file
operation completed at that instant. Only a measured `source.endSeconds` can
make that claim; omit it when only the operation's start was captured.

## Scene track

Each scene adds `title` and `screen`, both nonempty strings. `title` is the
human-facing context, such as “About fbro”. `screen` is an extensible machine
identifier for the visible UI, such as `app.about`, `files.grid`,
`viewer.markdown`, or `git.diff.unified`.

A source chapter can contain multiple scenes. For example, its About modal,
Settings, and photo grid can each have their own scene event. IDs must remain
unique, even when the same screen is revisited.

## Operation track

Each operation adds:

| Field | Type | Meaning |
| --- | --- | --- |
| `sceneId` | string | Existing scene event containing this operation. |
| `text` | nonempty string | Audience-facing description, e.g. “Copy six selected photos”. |
| `commandId` | string, optional | Backend command ID when applicable, e.g. `file.copy`. |

Use concise descriptions of visible actions. Selection, confirmation, and
completion can be separate operations if they need different captions.
An operation need not have a command ID: clicking a control, typing a commit
message, or waiting for a real copy to finish are also operations.

## Keyboard track

Each keyboard event adds:

| Field | Type | Meaning |
| --- | --- | --- |
| `operationId` | string | Existing operation event containing this badge. |
| `key` | string | Native helper's physical-key token, e.g. `p`, `=`, `Enter`, `ArrowDown`, or a single space for Space. |
| `modifiers` | array of strings | Unique modifiers in canonical order: `ctrl`, `alt`, `cmd`, `shift`; `[]` for none. |
| `label` | nonempty string | Ready-to-render text, e.g. `⌘⇧P`, `Space`, or `⌘+Plus ×3`. |
| `repeat` | positive integer | Number of actual key presses represented. |
| `presses` | array | One observation per press, ordered by frame. |

Each press has `frame` (an integer inside the badge's display interval) and
`sourceSeconds` (its original clip-relative observation time, inside the
source clip). The number of presses must equal `repeat`. Different source
presses may map to the same output frame at high speed.

For Cmd+Plus, preserve the actual input as `key: "="` with
`modifiers: ["cmd", "shift"]`; use the audience-friendly `label` to display
`⌘+Plus`. Do not infer a physical chord by parsing a label.

The renderer consumes `label` literally. A command-log event and the physical
key event it causes represent one action, not two badges. Show typed sentences
and paths through operation captions rather than a badge for every character.

Recommended authoring defaults, not validity requirements:

- Keep a keyboard badge visible for at least 24 output frames (0.8 seconds at
  30fps), bounded by its operation and scene.
- Group consecutive identical chords within the same operation when their
  source timestamps are at most one second apart. Keep every press in
  `presses`, with a label such as `↓ ×6`.
- If distinct badges collide, shorten the earlier display interval or summarize
  the sequence in the operation caption. Do not shift the recorded press times.
- Avoid carrying captions or badges across a scene cut simply to reach a
  preferred display duration. Animation style, placement, fonts, colors, and
  entry/exit easing belong to the renderer, not this timing contract.

## Existing montage report: current output

Today `montage.mjs` writes `<video-name>.json` and `<video-name>.md` beside
the MP4. This report has no `format` or `version` discriminator and is not a
version-1 overlay timeline. Its current JSON fields are:

| Field | Meaning |
| --- | --- |
| `sourceRun`, `sourceDirectory` | Recording/collected-run ID and absolute input directory. |
| `file` | Absolute output MP4 path. |
| `speed` | Acceleration factor. |
| `durationSeconds`, `sourceDurationSeconds` | Probed output duration and summed source durations. |
| `width`, `height` | Encoded video dimensions. |
| `fps` | FFprobe frame-rate string, currently `30/1`. |
| `audio` | `false`. |
| `chapters[]` | Source clips in montage order. |

Each chapter has `id`, `title`, `source` (absolute clip path),
`sourceDurationSeconds`, `startSeconds`, `endSeconds`, and `actions[]`.
Chapter start/end seconds already refer to the accelerated montage.

Each action has `label`, `atSeconds` (accelerated montage-relative start),
and `sourceAtSeconds` (original clip-relative start). Neither action end times
nor screen context nor keyboard observations are currently exported here.
The recorder logs action starts before dispatching the associated input;
these are not confirmations of a visible result or operation completion.

Raw per-clip `*-logs/input.jsonl` entries contain `at` (UTC wall time), `kind`
(`command`, `key`, or `type`), and kind-specific fields. `key` entries contain
`key` and `modifiers`; `command` entries contain `id` and `binding`; `type`
entries contain `text` and `replace`. These logs do not have an explicit
wall-clock-to-video-start anchor. Keyboard overlays reconstructed from them
must be marked `estimated`, even though semantic action starts already have
recording-relative times in the manifest.

To produce recorded keyboard timing in future captures, record monotonic
clip-relative input times using the same start clock as `Scene.step`, plus
operation IDs, screen transitions, and observed operation end times.
