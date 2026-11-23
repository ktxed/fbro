# Skip problematic items in copy / move / delete

**Status:** implemented and reviewed. The review findings are fixed, and the
Rust tests, the `tsc` build and the smoke tests (focused and full) pass. Not
committed yet. macOS and Windows have not been checked.

## Context

Before this change, a file operation stopped at the first error it couldn't
handle (`src-tauri/src/jobs/file_ops.rs`):

- **What existed:** the "file already exists" prompt (Overwrite / Resume / Skip
  / Cancel, plus "Apply to all"), the verification prompt (Retry / Skip /
  Abort), and a progress **Skip** button that worked for **copy only**.
- **What failed the whole job:** opening the source, creating the destination
  or its folder, write/flush errors, listing a subfolder, creating a symlink, a
  failed move rename, read errors after auto-resume gave up, and every delete
  error. Delete removed each top-level item with a single `remove_dir_all`.
- **Data-loss bug in Move:** a folder source was removed with
  `remove_dir_all` even when some of its children had been skipped. A single
  file's source was removed even when its copy returned "not copied". Errors
  while removing sources were ignored.

**Goal:** an error on one item prompts Retry / Skip / Skip all / Cancel. The
start dialog has an **Action on error: Ask / Skip** choice. Skip works during
moves. Move never deletes a source that wasn't fully transferred. The finished
job lists what was skipped.

**Decisions:** the conflict prompt keeps its "Apply to all" checkbox and gets
no "… all" buttons. The error policy is chosen in the start dialog.

## Backend

### Job state (`jobs/file_ops.rs`)
- `FileOpCtx<'a>` holds what stays constant for one job: job id, channels,
  drivers, policies, flags, and the skip/keep-partial `AtomicBool`s.
- `FileOpState` holds what changes: `decision_rx`, `apply_to_all`,
  `skip_all_errors`, the progress counters, and `skipped_count`. The count is
  uncapped; the list of skipped items lives on the job.

### Error policy and the error decision
- `ErrorPolicy { Ask, Skip }` (`jobs/mod.rs`) is stored in `FileOpJobMeta` and
  `QueuedFileOpLaunch`. `StartFileOp` takes the `errorPolicy` argument.
- `resolve_error`:
  - It returns Skip silently when the policy is `Skip` or after Skip all.
  - Otherwise it sends `DecisionRequired` with `kind: "error"` and the options
    `retry`, `skip`, `skip-all` and `cancel`.
- `attempt(ctx, st, src, dst, op)` runs one operation:
  - It retries the operation on Retry.
  - On Skip it records the item, sends `FileSkipped` and returns `None`.
  - Cancellation returns `Err(CANCELLED_ERR)`.

### Where errors can be skipped
- **File copy:** `CopyFileError` is split into `Cancelled`, `Io`,
  `Recoverable`, `Skipped` and `Verification`. `Io` errors and `Recoverable`
  errors, once auto-resume is used up, go through `handle_copy_error`:
  - **Retry** starts the file again from byte 0 and deletes nothing first; the
    next `open_write` truncates.
  - **Skip** deletes the destination only if this copy wrote to it
    (`dst_written` is set once `open_write` succeeds). A file the user chose to
    overwrite or resume survives an error raised before the first write. The
    progress Skip button follows the same rule.
- **Folders:** a failing `create_dir_all` or `read_dir_entries` skips the whole
  subtree. `copy_dir_recursive` returns `complete: bool`.
- **Symlinks:** read, create and place all go through `attempt`. When
  overwriting, the old destination is renamed aside
  (`{dst}.fbro-old-<uuid>`) while the new link is placed, and put back if that
  fails. Retry therefore works, and Skip keeps the original. Replacing a real
  folder with a link is refused.
- **Move rename:** a failing `try_rename` goes through `attempt`. If it is
  skipped, the item is counted and left in place. Only `Some(false)`
  (cross-device) falls back to copy + delete.
- **"Cannot place onto itself"** stays fatal, because it is a validation error.
- **Verification** keeps its own policy. With "Ask" it prompts even under the
  Skip error policy or after Skip all, so an unverified copy is never kept
  silently.

### Move removes only what was transferred
Each source file is removed right after its copy succeeds, through `attempt`.
A source folder is removed only when its subtree is `complete` and it is empty.
Skipped items keep their sources.

### Delete walks the tree
`delete_tree` reads entries, removes links as links (it never follows them),
deletes files one at a time through `attempt`, and removes a folder only if
nothing below it was skipped. Totals grow as folders are discovered. The
top-level folder entry doesn't add to `completed_files`, so progress never goes
past 100%.

### Reporting skipped items
- **Count and list:**
  - `FileOpProgress.skippedFiles` is the uncapped count.
  - `FileSkipped` carries `skipped: { path, reason, opType, notify }`.
  - `ws/server.rs` keeps the most recent 1000 items on `FileOpJobMeta.skipped`
    and adds 1 to the count for each event.
  - The list is not part of the connection snapshot, so a client that
    reconnects doesn't get it back.
- **Delivery:** `FileSkipped` and `DecisionRequired` are never dropped. When
  the channel is full they fall back to `blocking_send`, which is safe because
  the worker runs in `spawn_blocking`. `Progress` events may still be dropped.
- **Per-item toasts** (`notifications.rs`) appear only when `notify` is set: a
  Skip answered for that one item in the error, conflict or verification
  prompt, or the progress Skip button. The text is "Skipped copying / moving /
  deleting X". Skips from Skip all, Apply to all or a Skip policy are silent.
- **Completion toast** (`notifications/jobs.rs`): Copy, Move and Delete jobs
  that complete with skips publish a warning ending in ", N skipped".
- **Journal:**
  - `record_operation_finished` includes the skipped count.
  - A partial folder copy is still recorded.
  - A move whose source stayed behind is recorded as `copy`.

## Frontend

- **`src/ws/types.ts`:**
  - `ErrorPolicy` and `SkippedItem`.
  - The decision kind `"error"`.
  - `skippedFiles` on progress and `skipped` on the job event and on `Job`.
- **`src/store/index.ts`:** on `FileSkipped`, append the item (capped at 1000)
  and add 1 to `skippedFiles`.
- **`src/components/OperationModal.tsx`:**
  - **Start dialog:** a new "Action on error" select (`file-error-policy`),
    shown for copy, move and delete.
  - **Error prompt:**
    - Title "Cannot copy / move / delete item:", then the path and the message.
    - Retry is the primary button (Enter, initial focus), then Skip (`S`),
      Skip all (`L`) and Cancel.
  - **Progress:** the Skip button (`S`) shows for copy and move.
  - **Terminal:**
    - It says "Operation completed." when something was skipped and "…
      successfully." otherwise.
    - Below that: "N items were skipped", plus "Showing the last 1000" when the
      list is cut. Then the list of paths and reasons and a `CopyButton` for
      the skipped paths.
- **`src/components/JobsModal.tsx`:** a "Skipped" chip.

## Specs, guide, tests

- **Specs:**
  - `spex/proto.md`: `errorPolicy`, the `error` decision kind, `skip-all`,
    `FileSkipped.skipped`, `skippedFiles`, the notify, delivery and destination
    rules, and Move's source safety.
  - `spex/vfs.md`: how errors map to Retry, Skip and Abort.
- **Guide** (`site/src/content/docs/files/operations.mdx`):
  - Covers "Action on error", the error prompt, Skip during moves and the
    skipped summary.
  - Says that verification still follows its own setting.
- **Rust unit tests** (`jobs/file_ops.rs`). With injected driver failures:
  - Retry, explicit Skip, Skip all, and the Skip policy.
  - A delete with one failing child, and symlinks to folders during delete.
  - Delete progress never exceeding the total.
  - Move source safety for conflict, progress and error skips, and for a full
    move.
  - A failed source removal.
  - A skipped move rename keeping the source.
  - A Skip before the first write keeping the existing destination.
  - Conflict Skip with Apply to all being silent.
- **Real permission errors, Unix only** (`tests::access`, skipped when run as
  root):
  - An unreadable source prompts, and Skip copies the rest.
  - Retry succeeds once access is restored.
  - A read-only destination folder with the Skip policy records every file.
- **Smoke:** `tests/e2e/smoke/cases/file-op-errors.mjs`, run on its own with
  `FBRO_E2E_FILE_OP_ERRORS_ONLY=1`.
  - A `chmod 000` source opens the error prompt with Retry focused, and the
    panel underneath ignores keys.
  - `S` skips; the terminal lists the item, the toast appears, and focus goes
    back to the panel.
- **Not tested automatically:**
  - The symlink overwrite restore.
  - The verification prompt under the Skip error policy.
  - Retry and Skip all in the UI.

## Manual check

fbro must not run as root, because root ignores permission bits.

```bash
# Copy/move: a source that can't be read
mkdir -p /tmp/fbro-test/src/tree /tmp/fbro-test/dst
echo blocked > /tmp/fbro-test/src/tree/blocked.bin
echo good    > /tmp/fbro-test/src/tree/good.txt
chmod 000 /tmp/fbro-test/src/tree/blocked.bin
# While the prompt is open, restore access and press Retry:
chmod 644 /tmp/fbro-test/src/tree/blocked.bin

# A destination folder that can't be written to
mkdir -p /tmp/fbro-test/dst/tree && chmod 555 /tmp/fbro-test/dst/tree

# Delete: a file can't be removed when its parent folder is read-only
mkdir -p /tmp/fbro-test/del/locked
touch /tmp/fbro-test/del/locked/a.txt /tmp/fbro-test/del/ok.txt
chmod 555 /tmp/fbro-test/del/locked

# Clean up
chmod -R u+rwX /tmp/fbro-test && rm -rf /tmp/fbro-test
```

To check Skip all, make several files unreadable and press `L` at the first
prompt. There should be no further prompts, and every file should be listed at
the end.

## Open items

- macOS and Windows haven't been checked. On Windows the main risks are locked
  files, read-only attributes, and renaming the old destination aside during
  a symlink overwrite.
- This file is a plan. `AGENTS.md` puts plans in `spex/plans/`, so either move
  it there or leave it out of the commit.
