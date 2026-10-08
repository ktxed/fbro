# Inline operation choices

The inline choices bar is a compact, context-sensitive toolbar rendered in the
cursor row of a list panel. It exposes useful operations without changing the
panel selection or opening the context menu.

## Operation-item model

The bar is composed from operation items. An item defines:

| Field | Meaning |
|---|---|
| `id` | Stable semantic identity, such as `file.copy` or `git.stage` |
| `label` | Short tooltip text |
| `ariaLabel` | Complete accessible name including the target |
| `icon` | Compact visual representation |
| `run` | Contextual invocation against the resolved entry or selection |
| `cardinality` | Inclusive `{ min, max? }` target count accepted by the operation |
| `danger` | Optional destructive-action styling |
| `armed` | Optional confirmation state for an action confirmed in place |
| `preview` | Optional rich tooltip payload, currently a thumbnail URL and file name |

Rendering, event isolation, tooltips, focusability, and test attributes belong
to the shared operation-item renderer. Context providers decide which items
exist. New contexts and operations should add items rather than new button
rendering branches.

## Context composition

The inline bar and the `c` context menu share one contextual target resolver.
If the cursor or clicked row belongs to a non-empty selection, that complete
selection is the target. Otherwise the row itself is the target. A
multi-selection bar is rendered once in the cursor row and labelled `N selected`.
Singleton directories do not render the bar: Right Arrow is reserved for
drilling into them. Directories may still be part of a multi-selection target,
where applicable bulk operations remain visible.

The visible list is recomputed from the target, panel/source state, advertised
source capabilities, and operation cardinality. Inapplicable operations are
omitted, not shown disabled. Context lists are concatenated in this order:

1. status-specific operations (currently Git);
2. regular file operations.

This lets a modified Git file show one bar containing both Git and file
operations. Git operations are single-target only; a multi-selection gets only
the applicable general operations.

### Current items and eligibility

| Item | Cardinality | Eligibility / behavior |
|---|---:|---|
| Browse archive | exactly 1 | Backend-advertised navigable archive; always the first choice and activation enters the existing archive shadow source |
| Jump to location | exactly 1 | Search-result file with a real `sourcePath`; follows Browse archive, opens its containing folder in the same panel, and places the cursor on the file |
| View Git diff, Stage / Unstage, Rollback | exactly 1 | Single changed Git file; rollback retains its in-place confirmation |
| Preview | exactly 1 | Image file on a thumbnail-capable source; hover/focus shows the thumbnail and activation opens the viewer |
| Compare | 1–2 | Exactly two selected files, or a single selected file that differs from the file under the cursor — the own panel's lone selection, otherwise the other panel's single selection (a cross-panel pair with one selection per panel) |
| Pass to Bro | 1+ | Supported panel target; archive providers remain choices inside Bro |
| Copy / Move / Delete | 1+ | Source advertises the corresponding capability; normal confirmation still applies |
| Rename | exactly 1 | Mounted local filesystem advertises `Rename`, or global-selection root |

Context-menu-only operations use the same model: Open With and platform reveal
accept exactly one item. Open With also accepts path-backed search-result
files and directories, using their exact `sourcePath`; platform reveal retains
its mounted-local-filesystem eligibility. Adding to a global selection accepts one or more.
Copying names or full paths accepts one or more and uses newline-separated
values with plural menu labels for a selection.

## Interaction and accessibility

- The choices bar has `role="toolbar"`; every item is a native button with an
  accessible target-specific name.
- From the panel, Right Arrow enters the first choice without moving the cursor. On a navigable archive this focuses Browse archive; Enter or Space then enters the archive.
- Inline operations that navigate away from the current row restore focus to the panel before dispatching the navigation, so keyboard input continues after the row unmounts.
  If a backend update removes a focused choice, its ref cleanup schedules one
  handoff to the listing. A newly focused modal, menu, or control keeps focus.
  Shortcuts opening a temporary surface preserve the choice as that surface's
  opener; they do not first reset focus to the panel.
  A singleton directory has no bar, so Right Arrow drills down. When a directory
  is the cursor row of a multi-selection, Right Arrow enters the bulk-operation
  bar instead; Enter still drills into the directory.
- Left/Right Arrow moves between choices. Right wraps from the last choice back
  to the first. Left moves to the previous choice; on the first choice it leaves
  the bar and returns focus to the panel, so a following Left acts as the normal
  panel keypress (navigate to the parent folder).
- Enter or Space invokes the focused choice.
- Escape returns focus to the panel. Up/Down also leave the bar and resume
  panel navigation. If the Preview tooltip is open, adjacent Up/Down navigation
  keeps it open across consecutive previewable images, re-anchors it to the new
  cursor row, and shows the loading throbber until the new thumbnail is ready.
  Reaching a non-previewable entry or using any other navigation closes it.
- Pointer events on a choice must not select, open, or drag the underlying row.
- Destructive operations retain their normal confirmation rules. Only an
  explicitly designed in-place confirmation may use `armed` state.
- Item identifiers and ordering are stable enough for end-to-end assertions;
  tests should prefer the semantic `data-inline-operation` value.

The inline bar is a shortcut surface, not a replacement for the command
palette or context menu. Operations that cannot fit these targeting and
interaction rules should remain in those broader surfaces.
