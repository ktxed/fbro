# Modal Keyboard and Resizability Audit

Baseline: commit `aec5bee` (2026-09-20). The inventory below records the
pre-remediation behavior found at that baseline.

Remediation status: implemented on the `modals-consolidation` branch.

- unsafe modal-level Enter handling and duplicate input submissions were
  removed;
- active-job Escape now backgrounds the view without cancelling the job;
- list dialogs and tablists use one composite Tab stop with arrow navigation;
- modal controls have visible focus, and named radios retain native group Tab
  behavior;
- Search mnemonics, pairing focus, and async Bro/History key ownership were
  corrected;
- all dialog semantics now come from `ModalDialog`, including
  `ResizableModal`; and
- resizing remains an explicitly pointer-only enhancement with a visible
  corner affordance and persisted size.

The verdict tables are retained as the evidence and rationale for the changes,
not as a description of the remediated implementation.

Default-focus follow-up (2026-10-01): all current modal call sites were reviewed.
The current per-dialog defaults are recorded in [Modal Stack §6](modal-stack.md#6-default-focus-and-actions).
Operation dialogs no longer overwrite the stack's focus with a delayed overlay
focus. Main operation, Background, decision, Keep Running, Close, and dry-run
Close targets are explicit, including in-place phase transitions. Permissions,
Settings, global-selection prompts, Search, and Git now declare their initial
target without a competing mount focus effect. Pairing arrival is handled by
the top Settings layer. The shared Tab cycle excludes negative-tabindex and
disabled controls; the default resolver skips hidden and disabled targets.
Global-selection name Enter submits once, and operation mnemonics remain
available when an action button owns focus.

This document audits every full-screen dialog rendered through `ModalLayer`.
That is the project-defined boundary for a modal: context menus, history
dropdowns, bookmark popovers, inline pairing confirmation, and separate
viewer/diff windows are not independent modal layers. The inline pairing flow
is covered under Settings because it changes that modal's focus behavior.

The inventory contains 23 `ModalLayer` call sites in 21 components. Thirteen
of those call sites use `ResizableModal`.

## 1. Expected behavior

The baseline comes from [Modal Stack](modal-stack.md) and
[Keyboard & Input Ownership](keyboard.md):

- the top modal owns keyboard input;
- focus starts inside it and cannot escape it;
- `Tab` and `Shift+Tab` visit enabled, visible controls in a predictable order;
- `Escape` closes the modal, or first dismisses a clearly nested modal state;
- `Enter` activates the focused control, except where a text field has a
  documented default-submit behavior;
- arrows and accelerators do not interfere with text editing;
- focus is restored after the modal closes; and
- content-heavy dialogs are resizable, with their size retained.

`ModalStackProvider` provides a sound base for ownership and containment. It
captures `Tab` before component handlers, cycles through visible controls,
redirects escaped focus, and redispatches keys from the top layer when focus
temporarily points outside it. Consequently, component-level `Tab` branches
are redundant and never receive ordinary Tab events.

## 2. Inventory

Legend:

- **Good**: meets the relevant invariants without a material keyboard issue.
- **Partial**: usable, but has a correctness, predictability, or accessibility
  issue.
- **Fail**: a core keyboard path can perform the wrong action or not work.
- **R** / **F**: resizable / fixed-size.

### 2.1 Creation, input, and confirmation dialogs

| Modal | Initial focus | Keyboard behavior | Esc | Size | Verdict and findings |
|---|---|---|---|---|---|
| Rename | Name input; contents selected | Input and layer both submit on `Enter`; input history uses arrows | Closes | F | **Fail.** One Enter event can call `submit` twice. Tabbing to Cancel and pressing Enter invokes Rename instead of the focused button. |
| Create Directory | Name input | Same input/layer Enter pattern as Rename | Closes | F | **Fail.** Can send `MakeDir` twice; Enter on Cancel creates instead. |
| New File | Name input | Same input/layer Enter pattern as Rename | Closes | F | **Fail.** Can send `TouchFile` twice; Enter on Cancel creates instead. |
| Go To Path | Path input; contents selected | Input history; both input and layer submit on Enter | Closes | F | **Fail.** Can send navigation twice; Enter on Cancel navigates instead. |
| Generic Input / Password | Value input | Input history for non-password values; both input and layer submit on Enter | Closes | F | **Fail.** Can send `ProvideInput` twice; Enter on Cancel submits instead. |
| Set Permissions | Layer root | Enter applies globally | Closes | F | **Fail.** Enter on Cancel or a permission checkbox applies the mode instead of activating the focused control. |
| To Global Selection | Existing-selection select, or new-name input | Enter adds globally; Up from an empty new-name input selects the last existing selection | Closes | F | **Fail.** Enter on Cancel adds instead. Focus otherwise follows the current form mode. |
| Quit | Layer root | Enter confirms quit globally | Cancels and closes | F | **Fail.** Tabbing to Cancel and pressing Enter still confirms quit. The handler also omits `preventDefault`. |
| Reset remembered sorting | Cancel button | Native focused-button activation | Closes only the nested confirmation | F | **Good.** Safe initial focus and proper stack nesting. Enter activates Reset or Cancel according to focus. |

Fixed sizing is reasonable for these compact prompts. Their bodies scroll
where needed, although Set Permissions is the closest candidate for optional
resizing when font size or item summaries are large.

### 2.2 Selection and command dialogs

| Modal | Initial focus | Keyboard behavior | Esc | Size | Verdict and findings |
|---|---|---|---|---|---|
| Command Palette | Search input | Up/Down wrap logical selection; Enter runs it | Closes | R | **Partial.** Main workflow is good. Tab enters every result button, but logical selection does not follow focus; Enter can therefore run a different command than the focused row. |
| Open With | Dialog root via `initialFocusRef`; key handler is on the layer | Up/Down wrap selection; Enter launches it | Closes | R | **Good for the primary workflow; Partial overall.** The recent layer-owned handler fixes initial key ownership. Tab can still focus an option different from logical selection, after which Enter launches the selected rather than focused option. |
| Pass To Bro | Inner dialog root | Up/Down wrap selection; Enter opens parameters or launches; form fields intercept editing keys | Closes | R | **Fail.** Results arrive asynchronously while the handler is below the layer. If the layer root owns focus, arrows and Enter do not reach the inner dialog—the same failure pattern formerly present in Open With. Enter on a focused Cancel or option can also launch the logical selection instead. |
| Navigation History | Inner dialog root | Up/Down clamp selection; Enter navigates | Closes | R | **Partial.** Normally has synchronous rows, so keys reach the inner handler. Empty/timing states can leave focus on the outer layer. Tab and logical selection can diverge. |

All four lists render every option as a normal tab stop. This makes Tab traverse
potentially hundreds of rows and mixes two focus models: a logical selected row
for arrows, and DOM focus for Tab. These should use one model, preferably focus
on the input/container with options removed from sequential Tab order, or a
proper roving `tabIndex` that synchronizes focus and selection.

### 2.3 Information and overview dialogs

| Modal | Initial focus | Keyboard behavior | Esc | Size | Verdict and findings |
|---|---|---|---|---|---|
| About | Close button | Native Tab activation; no letter accelerator | Closes | R | **Good.** Close is explicitly focused and Escape works. |
| Info | Layer root | `I` or Enter closes; `P` copies full path | Closes | R | **Fail.** Enter on any focused copy button closes instead of copying. The useful `P` accelerator is visually indicated. |
| No editor available | Layer root | Enter closes | Closes | F | **Fail.** Enter on the focused Copy Path control closes instead of copying. |
| Jobs | First tab through stack fallback | `J` closes; other controls use native activation | Closes | R | **Partial.** Focus is contained and activation is safe, but the three visual tabs have no arrow-key tablist behavior and only `J` is advertised. |

### 2.4 Settings and search

| Modal | Initial focus | Keyboard behavior | Esc | Size | Verdict and findings |
|---|---|---|---|---|---|
| Settings | First sidebar tab; pairing flow moves focus to Confirm | Up/Down changes the active sidebar tab | Closes; also rejects unanswered pairing | R | **Partial.** Up/Down changes state without moving focus, so a second Down from General selects Display again rather than Sharing. All tabs remain in the Tab sequence instead of using roving tab focus. Security-sensitive pairing initially focuses Confirm rather than Cancel. |
| Search | File-mask input on input tab; dialog root on other tabs | `S`/`R`/`E` switch tabs outside form fields; Enter starts search or mounts results | Closes | R | **Fail.** Enter on Background, Cancel Search, To Results, or Close can run the modal's default action rather than the focused button. `B` and `C` are rendered as mnemonics for Background and Close but have no handlers. Tab also visits every radio rather than respecting native radio-group tab behavior. |

Search correctly avoids intercepting letter accelerators while focus is in an
input, select, or textarea. Settings correctly keeps the pairing confirmation
inside the Settings layer rather than adding an unregistered overlay.

### 2.5 Git

| Modal | Initial focus | Keyboard behavior | Esc | Size | Verdict and findings |
|---|---|---|---|---|---|
| Git status/log | Close button when changes exist; first log item for log-only repositories | Rich navigation: arrows/J/K, Home/End, Left/Right tree movement, Space selection/load-more, `1`/`2` tabs, `B/F/L/P/R/U/C`, `V/S/Y`, and Ctrl/Cmd+A | Collapses rollback or expanded commit first, then closes | R | **Good, with cleanup needed.** Form controls retain native editing. The modal's local Tab trap duplicates and is pre-empted by the stack trap. Both `ModalLayer` and `ResizableModal` declare dialog semantics, creating nested dialogs for assistive technology. |

Git is the strongest modal-specific keyboard implementation. Its staged Escape
behavior is a reasonable nested-state exception to “Escape closes” because the
first press visibly dismisses rollback confirmation or an expanded commit.

### 2.6 File operation and job dialogs

`OperationModal` has three mutually exclusive layer call sites and must be
audited as three dialogs.

| Modal/phase | Initial focus | Keyboard behavior | Esc | Size | Verdict and findings |
|---|---|---|---|---|---|
| Operation confirmation | Layer root; conflicting destination name is selected when applicable | Enter starts foreground operation; `A` cycles conflict action, `D` toggles dry mode, `P` start-paused, `B` background | Closes confirmation | R | **Fail.** Enter is global for every non-input control, including Background, Queue, and Cancel buttons, so it can start the foreground action instead of the focused action. |
| Active/queued job | Layer root, refocused on phase changes | Enter backgrounds/closes progress; Escape begins cancellation; `R` resumes paused jobs | Requests cancellation rather than closing while active | R | **Fail against the stated invariant.** Focused Pause, Resume, Skip, and Cancel buttons do not own Enter. Escape changes job state or opens cancellation rather than merely dismissing the modal. |
| Conflict decision | Layer root, refocused for each decision | Enter chooses primary decision; `S` skips; `A` toggles apply-to-all | Cancels the job | R | **Partial/intentional risk.** Efficient for keyboard operation, but Escape is destructive and does not close. It needs an explicit documented exception and safer wording/focus behavior. |
| Terminal job/result | Layer root | Enter or Escape closes/backgrounds; result action buttons use native click only if activated by pointer/Space | Closes or acknowledges | R | **Fail.** Enter on Save Report, Show Result, Copy Path, or other result actions closes instead of activating the focused button. |
| Dry-run result | Layer root | Native focused-button activation | Closes | R | **Good.** Escape and Close are predictable; path/copy buttons retain Enter behavior. |

The active-job Escape policy conflicts with the requested universal invariant.
If cancelling is intentional, Escape should first close/background the view;
cancellation should have its own explicit command. At minimum, the exception
must be specified and visibly communicated.

## 3. Cross-cutting findings

### P0 — action correctness

1. **Focused buttons frequently do not own Enter.** Layer or dialog handlers
   treat Enter as a default action without checking whether the target is a
   button. This can turn Cancel into Rename/Create/Quit, Background into a
   foreground operation, or Copy into Close.
2. **Five input dialogs can submit twice.** Rename, Create Directory, New File,
   Go To Path, and Generic Input handle Enter on both the input and its
   `ModalLayer`. The input prevents the browser default but does not stop
   propagation, and the layer does not check `defaultPrevented`.
3. **Key ownership must live at a reliable boundary.** Open With now handles
   keys on `ModalLayer`; Pass To Bro and, less severely, Navigation History
   still rely on a focusable inner descendant receiving the event.

Default-action handlers should return immediately for buttons, links, and form
controls unless that control explicitly delegates Enter. Input handlers should
be the only submit owner while an input is focused, or the layer should be the
only owner and check `defaultPrevented`.

### P1 — navigation and discoverability

1. **Keyboard focus is usually invisible.** `src/styles.css` globally removes
   `:focus` and `:focus-visible` outlines. Text inputs and Git rows have custom
   focus styling, but most modal buttons, tabs, checkboxes, selects, and list
   options do not. A keyboard user cannot reliably tell what Tab selected.
2. **The global Tab trap flattens composite controls.** Every enabled button,
   input, select, textarea, link, and non-negative `tabIndex` is included. This
   bypasses native radio-group Tab behavior and makes large result lists part
   of the sequential order.
3. **Tablists are incomplete.** Settings does not move focus with selection;
   Search and Jobs do not implement standard arrow navigation or roving
   `tabIndex`.
4. **Accelerator presentation and behavior diverge.** Search underlines `B`
   and `C` without implementing them. Other common buttons rarely expose
   accelerators, while Git and Operation confirmation do so consistently.
5. **Escape semantics are inconsistent.** Most dialogs close, Git dismisses a
   nested state first, and active jobs cancel or prompt to cancel. These
   differences should be deliberate, documented, and visible.

### P2 — semantics, resizing, and maintainability

1. **Dialog semantics are inconsistent.** Several fixed dialogs have no
   `role="dialog"`; several dialogs omit `aria-modal`; Git declares nested
   dialogs. `ResizableModal` should supply the semantics once, and fixed modal
   shells should use an equivalent shared primitive.
2. **Resizing is mouse/pointer-only.** Eight invisible edge/corner `div`s are
   `aria-hidden` and have no keyboard operation or visible affordance. Size
   clamping and persistence through `SetModalSizePreference` otherwise work
   consistently.
3. **Local Tab handlers are dead or redundant.** Because the provider consumes
   Tab in capture phase, modal handlers that prevent Tab—including Git's full
   second focus-trap implementation—should be removed after the shared trap is
   made composite-control-aware.

## 4. Remediation roadmap

### Phase 1: make actions safe

- Introduce a shared predicate for interactive/form targets and use it before
  modal-level Enter or printable accelerators.
- Give each input submission one handler; honor `defaultPrevented` and stop
  propagation only where a child deliberately owns the key.
- Move Bro and History navigation handlers to `ModalLayer`, and provide an
  explicit initial focus ref where logical selection owns arrows.
- Change active job Escape to close/background the view, or formally amend the
  invariant and add an explicit, visible cancellation cue.

### Phase 2: normalize keyboard navigation

- Restore a visible `:focus-visible` treatment for every modal control.
- Use roving focus for tablists and selectable lists; remove bulk result rows
  and unchecked radio siblings from sequential Tab order.
- Make Settings, Search, and Jobs tabs follow ARIA tab keyboard behavior.
- Implement every displayed mnemonic or remove its mnemonic styling. Reserve
  Enter for native focused-control activation whenever possible.
- Focus Cancel for security-sensitive pairing and destructive confirmations.

### Phase 3: consolidate modal primitives

- Provide one shared dialog shell for role, label, `aria-modal`, initial focus,
  and optional resizing.
- Remove duplicate component-level Tab traps and nested dialog roles.
- Add a visible resize affordance and a documented keyboard resize mechanism,
  or explicitly classify resizing as a pointer-only enhancement.

## 5. Test coverage

The baseline smoke coverage verified parts of the contract: About restores panel
focus, Settings reset nesting handles Escape, Git owns navigation keys, and
Search/Git prevent global modal shortcuts from opening above them. It does not
systematically test focused-button Enter behavior, Tab order, initial focus,
or every Escape path.

Add reusable modal-contract checks for each modal archetype:

1. opening places `document.activeElement` inside the top layer;
2. Tab and Shift+Tab remain inside it and show a visible focus indicator;
3. Enter activates the currently focused button, especially Cancel and
   secondary actions;
4. a submit input emits exactly one intent;
5. Escape closes without mutating the underlying operation unless the
   documented exception is under test;
6. list arrows work immediately without a pointer click;
7. a nested layer leaves its parent inert and restores focus on close; and
8. resizing persists under the expected modal preference key.

The highest-value regression cases are Quit/Cancel, Rename/Cancel, one
single-submit input, Bro during loading-to-ready transition, Search
Background/Close, Operation Background/Cancel, terminal-result actions, and
Settings arrow navigation.
