# fbro — Keyboard & Input Ownership

Keyboard input belongs to the highest-priority active UI surface. This keeps a
covered modal, panel, or global shortcut from responding to a key intended for
the surface the user can currently interact with.

Modal stacking, focus containment, and restoration are specified in
[Modal Stack](modal-stack.md).

---

## 1. Input Priority

| Priority | Surface | Key handling |
|---:|---|---|
| 1 | Top modal layer | Own handler; focus is trapped within the layer |
| 2 | Context menu or other transient overlay | Overlay-specific handler |
| 3 | Focused panel or embedded viewer | Navigation and selection commands |
| 4 | Toolbar window listener | Global command bindings |

Only the highest applicable surface may act. Modal presence is derived from
the registered modal stack, rather than duplicated booleans for individual
dialog types.

---

## 2. Current Implementation

Settings Sharing's folder input uses Enter to add the entered path exactly once;
its suggestion select keeps native keyboard behavior. Alt+Down opens existing
input history, whose arrows/Enter select a path and whose Escape closes history
before Settings. Add hands focus to the path input before submission; unchecking
a focused user-added folder does likewise before its row is removed. Replies
and sharing patches do not move focus. Covered Settings controls remain inert.

- `ModalStackProvider` captures key and focus events and routes them to the top
  `ModalLayer`.
- `Layout` passes `hasActiveModal || pendingContextMenu || pendingSourceMenu || bookmarksOpen` to panels as their
  modal guard. A guarded panel suppresses keyboard actions and focus intents.
- `Toolbar` uses the same conditions to suppress modal-opening and file-command
  shortcuts. Zoom shortcuts remain global.
- Settings → Display's **Show bottom button bar** checkbox and the command
  palette's **Toggle Button Bar** command control persistent visibility. There
  is no default binding for the toggle. `Toolbar` stays mounted when hidden,
  retaining global shortcut handling and all existing input guards; only its
  visible buttons are omitted. Closing Settings or the palette restores its
  opener through the modal stack as usual.
- Each modal owns its feature-specific keys on its layer or descendants.
- Single-item Copy confirmation uses `N` to focus and select the destination
  name for editing. The label displays the mnemonic; it does not intercept
  typing in editable fields.
- Multi-file search-result Copy uses `U` to focus Repeated filenames. The native
  selector owns its selection keys; changing the policy does not submit.
- The bookmark popover initially focuses the add/remove current-folder action
  (`+`/`−`). It is the first navigation position, followed by the saved bookmarks.
  Home focuses that action; End selects the last bookmark (or the action when
  the list is empty). Up/Down cycle through all positions: ArrowUp from the
  action selects the last bookmark, ArrowDown from the action selects the first
  bookmark, and ArrowDown from the last bookmark returns to the action. An empty
  list keeps focus on the action. Adding/removing a bookmark returns focus to
  the action. The list has no active bookmark while the action is focused.
  Enter opens the active list bookmark, while Enter or Space activates the
  focused current-folder button natively. Escape closes the
  popover; while it is open the underlying panel and global commands are held
  back.
- Embedded terminal keys go to the PTY for the shell to interpret. On macOS,
  fbro can start interactive zsh sessions with Emacs bindings when the backend
  `zshEmacsKeymap` setting is enabled; it passes `-o emacs` at launch. The
  setting is hidden and has no effect on other platforms. It does not inject
  text into the PTY or change a running session.
- Tab and Shift+Tab cycle among enabled, visible controls in the top modal,
  except inside a list marked `data-modal-owns-tab` (Pass to Bro), where they
  move the selection like ArrowDown/ArrowUp (`modal-stack.md` §4).
- Enter belongs to the focused native control. A modal-level default action
  may run only when the event target is not another interactive control and a
  child has not already consumed the event.
- Modal tablists and selectable lists use roving/composite focus: one Tab stop,
  arrows within the component, and synchronized focus and selection.
  Jobs omits Thumbnails for native OS providers; arrows and Home/End navigate
  only the views enabled by the backend's `supportsManagedThumbnails` capability.
- Every modal declares its default focus according to `modal-stack.md` §6.
  In-place workflow changes use `ModalLayer.focusKey`; progress updates do not
  steal focus. Negative-tabindex controls are excluded from the shared Tab
  cycle, and Enter on a focused secondary action keeps its native behavior.
- The removable-source picker is a `ModalLayer` opened from the command
  palette or source context menu. Its native list control owns selection keys;
  Escape closes and restores focus. The source menu owns arrows, Enter, and
  Escape while open.
- Enter and ArrowRight on a local directory symlink open its resolved target
  as a panel location. F3 on a local file symlink views the target file; a
  broken symlink reports an error instead of opening a viewer.

---

## Panel selection and folder sizes

Panel tab reordering is mouse-only. During an active tab drag, Escape cancels
the gesture before reaching the listing or PTY, without changing focus or
activating the dragged tab. A modal/menu opening cancels the drag and retains
the existing input priority. Clicking and the existing tab navigation commands
keep their normal behavior. See [Tabs](tabs.md) for the drag contract.

In list and grid listings, `+` opens **Select Files** and `-` opens
**Deselect Files**. The registry commands are `panel.selection.select_by_mask`
and `panel.selection.deselect_by_mask`; symbol matching includes Shift-produced
and numpad symbols, while Ctrl/Cmd zoom keeps its existing bindings. Held-key
repeats do not open or reopen the prompt. Text inputs, embedded terminals,
menus, and covered panels retain ownership of their keys.

The mask input initially has focus and selected text. Its default is the cursor
file's final extension (`*.txt`), or `*.*` for directories, extensionless names,
and empty listings. An unloaded cursor entry is fetched with `RequestWindow`.
Confirmation waits for the listing and initial lookup to be ready. Enter submits
once; Enter on Cancel cancels. Escape dismisses input history first, then the
dialog, returning focus through the shared modal stack. The command palette
also exposes both actions.

Patterns are whitespace-separated, case-insensitive whole-filename matches;
`*` matches any text and `?` one character. `*.*` matches all files, including
extensionless files. Select adds matches and Deselect removes matches from the
complete backend listing, including outside render windows, without changing
the cursor or unrelated selections. Directories and directory symlinks never
match; regular files and known file symlinks do. Existing selected directories
remain selected. Hidden and filtered-out entries are outside the operation.

In both list and grid listings, `Space` toggles the cursor entry's selection
without moving the cursor. When it selects a real folder with no displayed size,
it also starts a size calculation unless one is already queued or running.
Known sizes, including zero, are reused. Deselecting a folder leaves its size
calculation running. Mouse selection, range selection, and Select All do not
automatically calculate sizes.

`Shift+Space` leaves selection unchanged and toggles size calculations for the
selected folders, or all folders in the current listing when nothing is selected.
Running or queued calculations are cancelled; idle folders are freshly calculated
even if their size is already displayed. Targets come from the complete backend
listing, including entries outside the frontend render window. Jobs also provides
per-folder cancellation. Holding either shortcut does not repeat its action.

Size calculation supports local filesystem folders and ZIP/7z directories.
Other sources and directory symlinks remain selectable without an automatic scan.
Calculation completion, failure, cancellation, or timeout never changes selection.
The existing input-ownership rules apply; quick-preview video Space retains its
play/pause action instead of selecting the video.

## 3. Quick preview and separate viewer

`Ctrl+F3` (macOS: the registry's `Cmd+3`) toggles quick preview in the other
panel slots. It leaves focus in the source listing and follows its backend
cursor and active tab. Tab enters the preview content; Tab/Shift+Tab traverse
content and header controls and return to the source listing at the boundaries.
Clicking the preview activates its panel through `SwitchFocus` without cancelling
native media pointer events or text selection.

Quick preview shares file loading and rendering with F3, but chooses its
renderer automatically. Markdown defaults to the rendered document. It has no
renderer selectors, DOS, word wrap, find, editing, file-navigation letters, or
maximize action; their shortcuts are also inactive. Scroll, image zoom/pan,
PDF and diagram controls, and native audio/video controls remain available.
Media Space toggles playback; Left/Right seek five seconds, bounded by the clip.
When quick preview displays a video, unmodified Space in its source listing
also plays/pauses that video, preserving listing focus and selection. Holding
Space does not repeatedly toggle playback. Other entries keep the usual selection
and optional folder-size action; inputs, native controls and covered panels keep their
own keyboard ownership.
PDF uses the same arrows, PageUp/PageDown and Home/End scrolling as text.
A native button keeps its Space/Enter activation without a second viewer action.

The header's **Open viewer** action and the registry's `file.view` binding open
the displayed backend-resolved file, rather than the hidden listing in the
preview slot. Quick preview remains open. Closing the separate window preserves
its available opener in the preview; closing quick preview returns to the source
listing. Escape first dismisses an expanded diagram, then closes the preview.

Viewer keyboard and copy handlers are scoped to their content root. Background
previews do not respond to keys from menus or modals. The source listing explicitly
delegates video Space to the shared playback action. Input history
handles Alt+Down only for its own focused input. Replacing a preview content
session hands focus through its persistent panel root without taking an outside
control's focus.

### Separate viewer window

The viewer uses number keys for renderer selection and letters for actions or
text transformations:

| Key | Action |
|---|---|
| `1` | Raw text; explicitly leaves Markdown preview |
| `2` | Image/media renderer |
| `3` | Hex |
| `4` | Markdown preview for `.md`, `.markdown`, and `.mdown` files |
| `M` | Toggle maximize/restore, matching the compare window |
| `S` | Toggle DOS/CP437 decoding in raw text mode |
| `W` | Toggle word wrapping in raw text mode |
| `F` | Open find; Markdown preview first returns to raw text |
| `Enter` / `Shift+Enter` | Next/previous occurrence while find is open, from its input or file content |
| `E` / `F4` | Edit a real local file |
| `J` / `K` | Next/previous file (down/up in listing order) |
| `Space` | Play/pause directly rendered local video or MP3 audio |
| `Left` / `Right` | Scroll text, Hex, Markdown or PDF horizontally by 80 pixels; seek directly rendered media backward/forward five seconds |
| `Esc` | Dismiss find history, then find, then close the viewer; leave native fullscreen first on macOS |

Markdown is a distinct presentation of text rather than a second simultaneous
toggle: only `1 Text` or `4 MD` is shown as active. When focus is in the find
input, printable viewer shortcuts are not intercepted.
Find navigation wraps at either end and preserves the current focus, including
when there are no matches. Enter on a focused button retains native activation
and acts only once. In the find input, history selection consumes Enter before
match navigation; Escape dismisses history before closing find. Closing find
returns focus to its available opener without overriding an outside focus move.
IME composition and Ctrl/Cmd/Alt+Enter do not navigate matches.
Left/Right keep native cursor movement in the find input. Horizontal scrolling
also works for large files with virtual vertical scrolling and in quick preview.

Mermaid diagrams in the Markdown preview have a hover toolbar (zoom out, zoom
in, fit, expand); `Ctrl`/`Cmd`+wheel over a diagram zooms it in place, while a
plain wheel keeps scrolling the document. Expand shows the diagram over the
whole viewer body, where the wheel zooms at the cursor and dragging pans.
While it is open it owns the keyboard, and other viewer shortcuts are held back:

| Key | Action |
|---|---|
| `+` / `=` / `-` | Zoom in / out |
| `0` | Fit the diagram to the viewer |
| Arrows | Pan |
| `M` | Toggle maximize/restore |
| `Esc` | Return to the document (the viewer stays open) |

`Ctrl`/`Cmd`+`=`/`-`/`0` keep adjusting the global font size.

The Git changes/log lists use `J` to move down and `K` to move up.
In the Git log, Tab visits the active commit/file row itself and its enabled
inline actions, then leaves the listing. Inactive rows and their actions are
excluded from Tab order. Shift+Tab reverses that order; entering from before
the listing reaches the active row. Arrows, J/K, Home/End, and PageUp/PageDown
move the active item, including expanded file rows and message-load controls.
History Load more is excluded from Tab order to avoid scrolling to it; Space
from a commit row still loads the next page, and the button remains clickable.
The diff viewer uses the same file order as the separate viewer: `J` opens
the next changed file, and `K` opens the previous changed file.
In the diff viewer, Up/Down jump between change blocks when there are multiple
blocks. With zero or one block (including an entirely new or deleted file),
Up/Down scroll by one line instead. Split panes scroll together. P/N always
select the previous/next change block; Home/End and PageUp/PageDown scroll.
Shift+J/Shift+K select the next/previous panel tab.

---

## 4. Design Rules

Empty folder listings focus the first available suggestion in the active panel
after loading completes. Suggestions form one roving Tab stop: Up/Down and
Home/End move between actions, while Enter/Space activate the native button
once without also running panel entry navigation. Tab still switches panels;
Shift+Tab retains the Next Source binding. Returning from the New File dialog
restores suggestion focus if the folder remains empty; when entries appear,
focus returns to the listing.
Create a new file is first, followed by Create a new folder where supported.
Returning from either creation dialog restores the suggestion that opened it;
if that opener is unavailable, the first available suggestion receives focus.
If navigation completes while an overlay covers the listing, that navigation's
initial suggestion focus waits until the listing is uncovered.
Inactive panels, modal-covered panels, filter inputs, and embedded terminals
must not lose focus to suggestions.

1. Derive ownership from rendered UI state; do not toggle an input mode in
   response to key presses.
2. A full-screen dialog must use `ModalLayer` and must not install its own
   window-level key listener.
3. Prevent browser defaults before returning from a handler that consumes a
   key.
4. Focus follows ownership: the focused panel owns panel input, and the top
   modal owns modal input.
5. A transient overlay outside the modal stack must participate in both panel
   and global-command guards.
6. Escape closes or backgrounds the top modal without changing an underlying
   job. A visibly nested confirmation or expanded state may consume the first
   Escape before the containing modal closes.
7. Every modal control must have a visible `:focus-visible` presentation.
8. Temporary focus owners capture their opener before moving focus and hand it
   back after removal. Bookmarks and context menus use `useFocusReturn`; modal
   layers use the stack. An unavailable opener falls back to the active listing
   or its empty-folder suggestion. A menu launching a dialog carries the
   original opener through the handoff. Outside clicks that deliberately focus
   another control keep that focus. Portaled owners register on their actual DOM
   mount, not a parent effect that may run before the portal exists. Menu actions
   hand focus to the opener before dispatching a successor dialog. Filter inputs
   remain openers for temporary surfaces; listing navigation explicitly leaves
   the editor, and the sort selector returns to it through its own close path.
9. Informational toasts and mouse activation of app toolbar/job-bar actions
   preserve existing focus. Toast arrival and expiry never move focus. A removed
   focused inline action hands focus to its listing. Panel activation and native
   window return remain explicit focus transitions; clearing an overlay guard
   alone does not refocus the panel, and there is no hover or polling recovery.

Jobs headers accept Enter/Right to expand the file list and enter its first
row. Right reenters an expanded list; Up/Down navigate its files, Home/End
reach the first/last file, and PageUp/PageDown move by a file page. Pagination
follows keyboard focus and keeps the active item visible. Left returns to the
parent job, then collapses its list. Escape from a file collapses the list
before dismissing Jobs. Tab visits the active file row, its enabled Copy and
Jump buttons, then the next job (or Close). Shift+Tab reverses that row/action
order, then goes to the preceding job (or the tablist). Inactive file rows and
their actions stay outside the Tab sequence. Header buttons retain normal Tab
access and native activation.

Each file with a backend-resolved location offers Copy path (`C`) and Jump to
location (`G`, or Enter on its row). Copy uses the shared clipboard feedback
and preserves the active row; Jump closes Jobs and reveals the file in the
active panel. Unavailable locations disable these actions. Job progress does
not move focus, and covered panels remain inert.

The `j` Jobs modal includes a Notifications tab. Its tablist uses Left/Right
and Home/End with one Tab stop. The history list also exposes one Tab stop;
Up/Down and Home/End move the active entry and keep it visible. Incoming
notifications preserve the entry being browsed. Clear history is a native
button, and Escape or `j` closes the modal through normal opener restoration.
Queued toasts never take focus; each expiry removes only its own active ID.

## Git repository initialization

`git.init` (Initialize Git repository…) has no default binding and is available
from the palette and single-directory local context menus. The palette targets
the current folder; the menu targets its directory entry. Its branch input
initially receives focus and Enter submits once. Input history consumes its own
keys first; Enter on Cancel closes without submission. Escape closes an idle
form or backgrounds a started initialization. Panels and global commands remain
blocked while the dialog is open, and the shared focus handoff restores its opener.

## Git branch name entry

The Git modal's branch picker is a custom searchable dropdown. `B` focuses its
trigger; Enter, Space, or ArrowDown opens it and focuses the search input.
The dropdown groups backend-provided branches under Local and Remote; remote
names include their remote prefix. Typing filters both groups by case-insensitive
literal substring. Group headings are not selectable. Up/Down and Home/End highlight results without moving input focus;
Enter switches to the highlighted branch once. Options are not separate Tab
stops. Escape dismisses the dropdown before other Git nested state. Tab or
Shift+Tab uses the modal's normal focus cycle and dismisses without selecting.
An outside click dismisses and preserves intentionally moved focus. The picker
cannot open or switch during Git activity; its trigger remains focusable so
checkout dismissal can return focus to it. Modified editing keys and IME
composition retain native input behavior.

Selecting a remote branch creates or reuses a local tracking branch. When the
backend requests another local name, the existing inline branch form opens with
the selected remote as its source. Enter submits once; failure retains the name
for retry. Cancel/Escape restores the branch picker, and successful checkout
closes the form and restores the refreshed picker. The picker is unavailable
while either branch form is open; its trigger remains focusable.

The Git modal's New branch action uses local `N` outside editable fields. It
opens an inline toolbar form and focuses the name input. Enter submits once;
input history consumes its own navigation and Enter/Escape first. Escape
collapses an idle form before closing the modal; during creation it closes the
modal and backgrounds the activity without cancelling. Cancel restores focus
to New branch; success restores focus to the refreshed branch selector.

Check hashes follows the Pass to Bro list/form keyboard flow. Enter activates
its options, Space toggles Save result, and Tab reaches Launch. Its results view
has one Close action; Enter/Escape dismiss it without cancelling a running job.
The result scroll region accepts keyboard scrolling, while the covered panel
and global commands remain inert. Cancellation is available through Jobs.

Generate hashes follows the Check hashes results keyboard contract: the
Overwrite existing checkbox uses native Space activation, launch occurs once
from its primary button, and the results modal initially focuses Close.
Enter/Escape dismiss the view without cancelling; completion retains focus and
Jobs remains the cancellation entry point.

Settings → General → Notifications first exposes an **About OS notifications**
info button. Focus, hover, or Enter/Space opens a tooltip containing the event,
permission, and development-build guidance. Escape dismisses it without closing
Settings or moving focus; Tab/blur dismisses it as focus moves to the next control.
It never takes focus, adds a modal layer, or installs a global keyboard handler.

The section exposes **Open notification settings** where
the backend supports it (macOS and Windows). Tab reaches the button after the delivery
checkbox; native Enter/Space activates it once. While opening, an aria-disabled
guard blocks duplicate activation without removing the focused control. Opening
system settings retains the fbro modal and its focused button so returning to
fbro resumes there; replies and inline errors do not move focus. In a bare macOS
or Windows development run the delivery checkbox is disabled, so Tab reaches the settings
button after the info control; explanatory text remains available through the tooltip.

## Configurable registry shortcuts

Settings → Shortcuts lists every backend registry command, including
unassigned commands. Search covers titles, IDs, categories, and formatted
shortcuts; All, Customized, and Unassigned filters narrow the list. The command
list uses one Tab stop with Up/Down and Home/End navigation. Selected-command
controls add, edit, remove, remove all, or restore default bindings. Registry
hints and dispatch always read the current effective registry.

Bindings contain an ordered `strokes` array plus `modes` (`list`, `grid`, or an
empty array for both). V1 validates exactly one stroke. Editing retains its
position; adding appends. Defaults remain platform-specific in `ws/commands.rs`.
The top recorder modal explicitly captures the next supported non-modifier key,
including Tab, Enter, and Escape, consumes its event, and then returns to normal
modal navigation. The capture surface claims Tab through `data-modal-owns-tab`.
Escape after capture cancels. Recording suppresses application shortcuts,
including zoom. Ctrl/Cmd plus, minus, equals, and zero, plus the fixed
ContextMenu / Shift+F10 context-menu aliases, are reserved and rejected
by the backend. Existing modal, text editor, terminal, and viewer ownership rules
continue to take precedence over customized application commands.

Conflicting changes require Override or Abort. Conflicts name every affected
command and overlapping view context. Override removes only overlapping
contexts from other bindings; a Both-views binding can become Grid-only.
Disjoint List/Grid bindings may share a combination. Overlapping duplicates
within one command are invalid. A stale revision rejects even an Override;
users must review and retry with the current registry. No dispatch or label
changes until the complete candidate settings have been saved successfully.

Viewer/dialog-local customization, multi-stroke dispatch, profiles, and
import/export remain deferred.

Dispatch and recording canonicalize `KeyA`–`KeyZ` and `Digit0`–`Digit9` to
physical letters/digits before matching. This resolves key/code ambiguity to
one stroke on remapped and non-Latin layouts, including Option-produced keys.
Symbols from other physical codes (for example `Equal`, `Backquote`, or numpad
symbols) use the produced symbol; an unshifted symbol binding also accepts
Shift-produced versions. A symbol on a physical digit is recorded as that digit
plus its modifiers. On layouts placing a default symbol on a letter/digit key,
users can record that physical combination explicitly. This deterministic rule
replaces the previous logical-or-physical matcher, which could dispatch two
different commands from one event depending on registry order. Shared frontend
and Rust conflict fixtures cover the canonical interpretation.

A stale editor never silently advances its revision or applies an old index to
a new array. It shows the current bindings and requires explicit review; edits
follow the original binding identity after reordering. If that original changed
or was removed, the user cancels and reopens the current binding instead.
