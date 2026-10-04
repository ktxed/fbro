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

- `ModalStackProvider` captures key and focus events and routes them to the top
  `ModalLayer`.
- `Layout` passes `hasActiveModal || pendingContextMenu || pendingSourceMenu` to panels as their
  modal guard. A guarded panel suppresses keyboard actions and focus intents.
- `Toolbar` uses the same conditions to suppress modal-opening and file-command
  shortcuts. Zoom shortcuts remain global.
- Each modal owns its feature-specific keys on its layer or descendants.
- Single-item Copy confirmation uses `N` to focus and select the destination
  name for editing. The label displays the mnemonic; it does not intercept
  typing in editable fields.
- The bookmark popover moves focus into its composite list. Up/Down and
  Home/End move the active bookmark, Enter opens it, and Escape closes the
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

## 3. Separate viewer window

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
| `E` / `F4` | Edit a real local file |
| `J` / `K` | Previous/next file |
| `Space` | Play/pause directly rendered local video or MP3 audio |
| `Esc` | Close the viewer, or leave native fullscreen first on macOS |

Markdown is a distinct presentation of text rather than a second simultaneous
toggle: only `1 Text` or `4 MD` is shown as active. When focus is in the find
input, printable viewer shortcuts are not intercepted.

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

---

## 4. Design Rules

Empty folder listings focus the first available suggestion in the active panel
after loading completes. Suggestions form one roving Tab stop: Up/Down and
Home/End move between actions, while Enter/Space activate the native button
once without also running panel entry navigation. Tab/Shift+Tab still switch
panels. Returning from the New File dialog restores suggestion focus if the
folder remains empty; when entries appear, focus returns to the listing.
Create a new file is first, followed by Create a new folder where supported.
Returning from either creation dialog to an empty listing focuses the first
available suggestion. Inactive panels, modal-covered panels, filter inputs, and embedded terminals
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
