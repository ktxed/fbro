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
- `Layout` passes `hasActiveModal || pendingContextMenu` to panels as their
  modal guard. A guarded panel suppresses keyboard actions and focus intents.
- `Toolbar` uses the same conditions to suppress modal-opening and file-command
  shortcuts. Zoom shortcuts remain global.
- Each modal owns its feature-specific keys on its layer or descendants.
- Tab and Shift+Tab cycle among enabled, visible controls in the top modal.

---

## 3. Design Rules

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
