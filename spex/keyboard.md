# fbro — Keyboard & Input Modes

## 1. Problem

Keyboard events flow through two independent paths:

1. **Panel `onKeyDown`** — navigation, selection, Tab for focus switching
2. **Toolbar global `window` listener** — F-key file operations

Without input mode gating, keystrokes intended for a modal (or no target at
all) leak through to panels and toolbar handlers. For example, pressing
Backspace during a running copy operation navigates the source panel.

---

## 2. Input Modes

At any point in time, exactly one input mode is active.

| Mode | Active when | Key targets |
|------|-------------|-------------|
| `panel` | No modal is open | Panel navigation, Toolbar F-keys |
| `modal` | `pendingOp` or `activeJobId` is set | Modal buttons, Escape to close |

Switching between modes is implicit — derived from application state, not
toggled manually.

---

## 3. Current Implementation

- **Panel**: receives `modalActive` prop; `handleKeyDown` calls
  `preventDefault()` and returns immediately when true; `onFocus` skips
  `SwitchFocus` intent when true.
- **Toolbar**: global keydown handler checks `state.pendingOp || state.activeJobId`
  and returns early when either is set.
- **Modal overlay**: receives focus via `useEffect` + `ref.focus()` on mount;
  traps Tab with `preventDefault()` on its `onKeyDown`.

---

## 4. Known Limitations / TODO

### 4.1 Tab focus cycling within modals

Tab currently does nothing inside a modal — it is fully suppressed. The
expected behaviour (to be implemented) is to cycle focus between interactive
elements within the modal (buttons, select dropdowns).

### 4.2 Escape key

Escape should close/cancel the modal in all phases. Not yet wired.

### 4.3 Enter key in confirmation phase

Enter could act as a shortcut for the primary action (e.g. start the copy).
Not yet wired.

### 4.4 Future modes

Additional modes will be needed as new features arrive:

| Mode | Trigger | Example keys |
|------|---------|-------------|
| `command-palette` | `:` or Ctrl+P | typing, arrows, Enter, Escape |
| `inline-rename` | F2 or similar | typing, Enter to confirm, Escape to cancel |
| `search` | `/` or Ctrl+F | typing, Enter for next match, Escape to close |

These should follow the same pattern: derive the active mode from state,
gate handlers accordingly.

---

## 5. Design Rules

1. **Mode is state-derived, not event-driven.** Never toggle a boolean on
   keypress — derive the mode from the reducer state so it stays consistent
   across re-renders and state patches.
2. **preventDefault early.** When a mode blocks a key, call `preventDefault()`
   before returning to stop browser defaults (Tab focus, Backspace navigation).
3. **Focus follows mode.** The active mode's root element should hold DOM focus.
   Panel mode → focused panel div. Modal mode → modal overlay.
4. **No stopPropagation across modes.** The Toolbar listener is on `window`,
   outside React's event tree. Stopping propagation in React does not affect it.
   Each handler must independently check the current mode.
