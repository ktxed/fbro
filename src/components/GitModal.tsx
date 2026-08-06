import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { IconArrowDown, IconArrowUp, IconCopy, IconGitCompare, IconLoader2, IconMinus, IconPlus } from "@tabler/icons-react";
import { useAppDispatch, useAppState } from "../store";
import type { GitChange, GitLogRef, GitRepoStatus } from "../ws/types";
import type { WsClient } from "../ws/client";
import { ResizableModal } from "./ResizableModal";

type GitTab = "changes" | "log";

/** Focus target in the Log tab: a commit header or a nested file change. */
type LogFocus =
  | { kind: "commit"; index: number }
  | { kind: "change"; commitIndex: number; changeIndex: number };

const LOG_PAGE_SIZE = 100;

export function GitModal({ panelId, git, client, onClose }: { panelId: string; git: GitRepoStatus; client: WsClient; onClose: () => void }) {
  const dispatch = useAppDispatch();
  const { pendingGitLog } = useAppState();
  const gitLog = pendingGitLog?.panelId === panelId ? pendingGitLog : null;

  const [tab, setTab] = useState<GitTab>("changes");
  const [message, setMessage] = useState("");
  const staged = useMemo(() => git.changes.filter(c => c.indexStatus !== " " && c.indexStatus !== "?"), [git]);
  const modalRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const branchRef = useRef<HTMLSelectElement>(null);
  const commitRef = useRef<HTMLButtonElement>(null);
  const selectAllRef = useRef<HTMLInputElement>(null);
  const changeRowRefs = useRef<Array<HTMLDivElement | null>>([]);
  const logRowRefs = useRef<Array<HTMLDivElement | null>>([]);
  const logChangeRowRefs = useRef<Record<string, Array<HTMLDivElement | null>>>({});
  const logLoadMoreRef = useRef<HTMLDivElement | null>(null);
  const lastGitRef = useRef(git);
  const logLoadedRootRef = useRef<string | null>(null);
  const [activity, setActivity] = useState<{ action: string; message: string } | null>(null);
  const [activeChangeIndex, setActiveChangeIndex] = useState(0);
  const [logFocus, setLogFocus] = useState<LogFocus>({ kind: "commit", index: 0 });
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(() => new Set());
  const [rollbackPaths, setRollbackPaths] = useState<string[] | null>(null);
  const selectionAnchorRef = useRef<number | null>(null);
  const canCommit = message.trim().length > 0 && staged.length > 0;
  const canPull = git.clean && git.behindCount !== undefined && git.behindCount > 0;
  const canPush = !!git.upstream && git.aheadCount !== undefined && git.aheadCount > 0;
  const act = (name: string, args: Record<string, unknown> = {}) => client.sendIntent(name, undefined, { panelId, ...args });
  const runAction = (name: string, message: string, args: Record<string, unknown> = {}) => {
    setActivity({ action: name, message });
    act(name, args);
  };
  const selectedChanges = useMemo(() => git.changes.filter(change => selectedPaths.has(change.path)), [git.changes, selectedPaths]);
  const selectedStaged = useMemo(() => selectedChanges.filter(change => isStaged(change)), [selectedChanges]);
  const selectedUnstaged = useMemo(() => selectedChanges.filter(change => !isStaged(change)), [selectedChanges]);
  const allChangesSelected = git.changes.length > 0 && selectedChanges.length === git.changes.length;
  const logCommits = gitLog?.commits ?? [];
  const expandedHash = gitLog?.expandedHash ?? null;
  const expandedCommitIndex = useMemo(
    () => (expandedHash ? logCommits.findIndex((c) => c.hash === expandedHash) : -1),
    [expandedHash, logCommits],
  );

  const requestLog = useCallback((opts: { skip: number; append: boolean }) => {
    const requestId = crypto.randomUUID();
    dispatch({ type: "REQUEST_GIT_LOG", panelId, requestId, append: opts.append });
    client.sendIntent("GitGetLog", undefined, {
      panelId,
      requestId,
      skip: opts.skip,
      limit: LOG_PAGE_SIZE,
    });
  }, [client, dispatch, panelId]);

  const expandCommit = useCallback((hash: string) => {
    const existing = gitLog?.commitShows[hash];
    const requestId = crypto.randomUUID();
    dispatch({ type: "REQUEST_GIT_COMMIT_SHOW", panelId, requestId, hash });
    if (!existing || existing.status === "error") {
      client.sendIntent("GitGetCommitShow", undefined, { panelId, requestId, hash });
    }
  }, [client, dispatch, gitLog?.commitShows, panelId]);

  const collapseCommit = useCallback(() => {
    dispatch({ type: "SET_GIT_LOG_EXPANDED", hash: null });
  }, [dispatch]);

  const viewCommitDiff = useCallback((hash: string, change: GitChange) => {
    act("GitViewDiff", {
      targetPath: change.path,
      commitHash: hash,
      ...(change.oldPath ? { oldPath: change.oldPath } : {}),
    });
  }, [act]);

  const copyRawFile = useCallback((hash: string, change: GitChange) => {
    if (change.indexStatus === "D") return;
    act("GitCopyRawFile", {
      commitHash: hash,
      path: change.path,
    });
  }, [act]);

  const armRollback = (paths: string[]) => {
    const uniquePaths = [...new Set(paths)];
    if (samePathSet(rollbackPaths, uniquePaths)) {
      setRollbackPaths(null);
      runAction("GitRollbackPaths", `Rolling back ${describePaths(uniquePaths)}…`, { paths: uniquePaths });
    } else {
      setRollbackPaths(uniquePaths);
    }
  };
  const toggleSelection = useCallback((index: number, extendRange = false) => {
    const path = git.changes[index]?.path;
    if (!path) return;
    setRollbackPaths(null);
    setActiveChangeIndex(index);
    setSelectedPaths(current => {
      const next = new Set(current);
      if (extendRange && selectionAnchorRef.current !== null) {
        const first = Math.min(selectionAnchorRef.current, index);
        const last = Math.max(selectionAnchorRef.current, index);
        for (let cursor = first; cursor <= last; cursor += 1) next.add(git.changes[cursor].path);
      } else if (next.has(path)) {
        next.delete(path);
        selectionAnchorRef.current = index;
      } else {
        next.add(path);
        selectionAnchorRef.current = index;
      }
      return next;
    });
  }, [git.changes]);
  const toggleAllChanges = () => {
    setSelectedPaths(allChangesSelected ? new Set() : new Set(git.changes.map(change => change.path)));
    selectionAnchorRef.current = allChangesSelected ? null : 0;
    setRollbackPaths(null);
  };
  const focusChangeRow = useCallback((index: number) => {
    if (git.changes.length === 0) return;
    const nextIndex = Math.max(0, Math.min(index, git.changes.length - 1));
    setActiveChangeIndex(nextIndex);
    requestAnimationFrame(() => {
      const row = changeRowRefs.current[nextIndex];
      row?.focus();
      row?.scrollIntoView({ block: "nearest" });
    });
  }, [git.changes.length]);
  const focusLogItem = useCallback((focus: LogFocus, opts?: { revealLoadMore?: boolean }) => {
    if (logCommits.length === 0) return;
    let next = focus;
    if (next.kind === "commit") {
      const index = Math.max(0, Math.min(next.index, logCommits.length - 1));
      next = { kind: "commit", index };
    } else {
      const commitIndex = Math.max(0, Math.min(next.commitIndex, logCommits.length - 1));
      const hash = logCommits[commitIndex]?.hash;
      const count = hash && gitLog?.commitShows[hash]?.status === "ready"
        ? gitLog.commitShows[hash].changes.length
        : 0;
      if (count === 0) {
        next = { kind: "commit", index: commitIndex };
      } else {
        const changeIndex = Math.max(0, Math.min(next.changeIndex, count - 1));
        next = { kind: "change", commitIndex, changeIndex };
      }
    }
    setLogFocus(next);
    requestAnimationFrame(() => {
      if (next.kind === "commit") {
        const row = logRowRefs.current[next.index];
        row?.focus();
        if (opts?.revealLoadMore && logLoadMoreRef.current) {
          logLoadMoreRef.current.scrollIntoView({ block: "nearest" });
        } else {
          row?.scrollIntoView({ block: "nearest" });
        }
      } else {
        const hash = logCommits[next.commitIndex]?.hash;
        const row = hash ? logChangeRowRefs.current[hash]?.[next.changeIndex] : null;
        row?.focus();
        row?.scrollIntoView({ block: "nearest" });
      }
    });
  }, [gitLog?.commitShows, logCommits]);

  /** Flattened log navigation: commits + nested files of the single expanded commit. */
  const moveLogFocus = useCallback((delta: number) => {
    if (logCommits.length === 0) return;
    type Flat = LogFocus;
    const flat: Flat[] = [];
    for (let i = 0; i < logCommits.length; i += 1) {
      flat.push({ kind: "commit", index: i });
      const hash = logCommits[i].hash;
      if (expandedHash === hash && gitLog?.commitShows[hash]?.status === "ready") {
        const changes = gitLog.commitShows[hash].changes;
        for (let c = 0; c < changes.length; c += 1) {
          flat.push({ kind: "change", commitIndex: i, changeIndex: c });
        }
      }
    }
    if (flat.length === 0) return;
    const currentIdx = flat.findIndex((item) => {
      if (item.kind !== logFocus.kind) return false;
      if (item.kind === "commit" && logFocus.kind === "commit") return item.index === logFocus.index;
      if (item.kind === "change" && logFocus.kind === "change") {
        return item.commitIndex === logFocus.commitIndex && item.changeIndex === logFocus.changeIndex;
      }
      return false;
    });
    const from = currentIdx < 0 ? 0 : currentIdx;
    const to = Math.max(0, Math.min(flat.length - 1, from + delta));
    focusLogItem(flat[to]);
  }, [expandedHash, focusLogItem, gitLog?.commitShows, logCommits, logFocus]);

  const copyLogHash = useCallback((hash: string) => {
    void navigator.clipboard.writeText(hash);
    dispatch({ type: "SHOW_TRANSIENT_HINT", message: "Copied commit hash" });
  }, [dispatch]);

  const handleClose = useCallback(() => {
    dispatch({ type: "CLEAR_GIT_LOG" });
    onClose();
  }, [dispatch, onClose]);

  useEffect(() => {
    runAction("GitRefresh", "Fetching and refreshing…");
  }, [client, panelId]);

  useEffect(() => {
    return () => {
      dispatch({ type: "CLEAR_GIT_LOG" });
    };
  }, [dispatch]);

  useEffect(() => {
    if (activity && git !== lastGitRef.current) {
      setActivity({ action: "complete", message: `${activity.message.replace(/…$/, "")} complete` });
      // After a successful commit (or other mutating action), refresh log if it was loaded.
      if (logLoadedRootRef.current === git.root && tab === "log") {
        requestLog({ skip: 0, append: false });
      } else if (logLoadedRootRef.current === git.root) {
        // Invalidate so next visit to Log re-fetches.
        logLoadedRootRef.current = null;
      }
    }
    lastGitRef.current = git;
  }, [activity, git, requestLog, tab]);

  useEffect(() => {
    setActiveChangeIndex((index) => Math.min(index, Math.max(0, git.changes.length - 1)));
    changeRowRefs.current.length = git.changes.length;
    const available = new Set(git.changes.map(change => change.path));
    setSelectedPaths(current => {
      const next = new Set([...current].filter(path => available.has(path)));
      return samePathSet(current, next) ? current : next;
    });
    setRollbackPaths(null);
  }, [git.changes]);

  useEffect(() => {
    logRowRefs.current.length = logCommits.length;
    setLogFocus((current) => {
      if (logCommits.length === 0) return { kind: "commit", index: 0 };
      if (current.kind === "commit") {
        return { kind: "commit", index: Math.min(current.index, logCommits.length - 1) };
      }
      const commitIndex = Math.min(current.commitIndex, logCommits.length - 1);
      const hash = logCommits[commitIndex]?.hash;
      const count = hash && gitLog?.commitShows[hash]?.status === "ready"
        ? gitLog.commitShows[hash].changes.length
        : 0;
      if (count === 0 || expandedHash !== hash) {
        return { kind: "commit", index: commitIndex };
      }
      return {
        kind: "change",
        commitIndex,
        changeIndex: Math.min(current.changeIndex, count - 1),
      };
    });
  }, [expandedHash, gitLog?.commitShows, logCommits]);

  // Lazy-load log when switching to the Log tab (or when root changes).
  useEffect(() => {
    if (tab !== "log") return;
    if (logLoadedRootRef.current === git.root) return;
    logLoadedRootRef.current = git.root;
    requestLog({ skip: 0, append: false });
  }, [tab, git.root, requestLog]);

  useEffect(() => {
    const frame = requestAnimationFrame(() => closeRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, []);

  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = selectedChanges.length > 0 && !allChangesSelected;
    }
  }, [allChangesSelected, selectedChanges.length]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        if (rollbackPaths) setRollbackPaths(null);
        else if (tab === "log" && expandedHash) {
          collapseCommit();
          const idx = expandedCommitIndex >= 0 ? expandedCommitIndex : 0;
          focusLogItem({ kind: "commit", index: idx });
        } else handleClose();
        return;
      }
      if (event.key === "Tab") {
        const modal = modalRef.current;
        if (!modal) return;
        const controls = Array.from(modal.querySelectorAll<HTMLElement>(
          'button:not([disabled]), select:not([disabled]), input:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        )).filter((control) => control.offsetParent !== null);
        if (controls.length === 0) return;
        const current = document.activeElement as HTMLElement | null;
        const index = current ? controls.indexOf(current) : -1;
        const next = event.shiftKey
          ? controls[(index <= 0 ? controls.length : index) - 1]
          : controls[(index + 1) % controls.length];
        event.preventDefault();
        event.stopImmediatePropagation();
        next.focus();
        return;
      }
      const target = event.target;
      // Form fields keep their normal editing/navigation behaviour. The
      // overlay below still stops their events from reaching the panel.
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return;
      const key = event.key.toLowerCase();
      const hasShortcutModifier = event.ctrlKey || event.metaKey || event.altKey;
      if (!hasShortcutModifier) {
        if (key === "1") {
          event.preventDefault();
          event.stopImmediatePropagation();
          setTab("changes");
          return;
        }
        if (key === "2") {
          event.preventDefault();
          event.stopImmediatePropagation();
          setTab("log");
          return;
        }
        const movement = event.key === "ArrowUp" || key === "k" ? -1 : event.key === "ArrowDown" || key === "j" ? 1 : 0;
        if (movement) {
          event.preventDefault();
          event.stopImmediatePropagation();
          if (tab === "log") moveLogFocus(movement);
          else focusChangeRow(activeChangeIndex + movement);
          return;
        }
        // Home / End: first and last visible log item (commit or nested file).
        if (event.key === "Home" || event.key === "End") {
          event.preventDefault();
          event.stopImmediatePropagation();
          if (tab === "log") {
            if (logCommits.length === 0) return;
            if (event.key === "Home") {
              focusLogItem({ kind: "commit", index: 0 });
            } else {
              // Last commit; if it is expanded with files, land on last file.
              const last = logCommits.length - 1;
              const hash = logCommits[last]?.hash;
              const changes = hash && expandedHash === hash && gitLog?.commitShows[hash]?.status === "ready"
                ? gitLog.commitShows[hash].changes
                : [];
              if (changes.length > 0) {
                focusLogItem({ kind: "change", commitIndex: last, changeIndex: changes.length - 1 }, { revealLoadMore: !!gitLog?.hasMore });
              } else {
                focusLogItem({ kind: "commit", index: last }, { revealLoadMore: !!gitLog?.hasMore });
              }
            }
          } else {
            if (git.changes.length === 0) return;
            focusChangeRow(event.key === "Home" ? 0 : git.changes.length - 1);
          }
          return;
        }
        if (tab === "log") {
          // Right: expand active commit / enter first nested change.
          if (event.key === "ArrowRight") {
            event.preventDefault();
            event.stopImmediatePropagation();
            const commitIndex = logFocus.kind === "commit" ? logFocus.index : logFocus.commitIndex;
            const commit = logCommits[commitIndex];
            if (!commit) return;
            if (expandedHash !== commit.hash) {
              expandCommit(commit.hash);
              focusLogItem({ kind: "commit", index: commitIndex });
              return;
            }
            // Already expanded: dive into first file when available.
            if (logFocus.kind === "commit") {
              const show = gitLog?.commitShows[commit.hash];
              if (show?.status === "ready" && show.changes.length > 0) {
                focusLogItem({ kind: "change", commitIndex, changeIndex: 0 });
              }
            }
            return;
          }
          // Left: leave nested change → parent commit; collapse when on header.
          if (event.key === "ArrowLeft") {
            event.preventDefault();
            event.stopImmediatePropagation();
            if (logFocus.kind === "change") {
              focusLogItem({ kind: "commit", index: logFocus.commitIndex });
              return;
            }
            const commit = logCommits[logFocus.index];
            if (commit && expandedHash === commit.hash) {
              collapseCommit();
              focusLogItem({ kind: "commit", index: logFocus.index });
            }
            return;
          }
          if (event.key === " " && logFocus.kind === "commit" && gitLog?.hasMore && gitLog.status !== "loading") {
            event.preventDefault();
            event.stopImmediatePropagation();
            requestLog({ skip: logCommits.length, append: true });
            return;
          }
          if (logFocus.kind === "commit") {
            const activeCommit = logCommits[logFocus.index];
            if (activeCommit && key === "y") {
              event.preventDefault();
              event.stopImmediatePropagation();
              copyLogHash(activeCommit.hash);
              return;
            }
          }
          if (logFocus.kind === "change" && (key === "v" || key === "c")) {
            const commit = logCommits[logFocus.commitIndex];
            const change = commit && gitLog?.commitShows[commit.hash]?.status === "ready"
              ? gitLog.commitShows[commit.hash].changes[logFocus.changeIndex]
              : undefined;
            if (commit && change) {
              event.preventDefault();
              event.stopImmediatePropagation();
              if (key === "v") viewCommitDiff(commit.hash, change);
              else copyRawFile(commit.hash, change);
              return;
            }
          }
        } else {
          const activeChange = git.changes[activeChangeIndex];
          if (activeChange && event.key === " " && target instanceof HTMLElement && target.matches('[data-testid="git-change-row"]')) {
            event.preventDefault();
            event.stopImmediatePropagation();
            toggleSelection(activeChangeIndex);
            return;
          }
          if (activeChange && key === "v") {
            event.preventDefault();
            event.stopImmediatePropagation();
            act("GitViewDiff", { targetPath: activeChange.path });
            return;
          }
          if (activeChange && key === "s") {
            const isStaged = activeChange.indexStatus !== " " && activeChange.indexStatus !== "?";
            event.preventDefault();
            event.stopImmediatePropagation();
            runAction(isStaged ? "GitUnstagePaths" : "GitStagePaths", `${isStaged ? "Unstaging" : "Staging"} ${activeChange.path}…`, { paths: [activeChange.path] });
            return;
          }
        }
      }
      const accelerators: Record<string, () => void> = {
        b: () => branchRef.current?.focus(),
        l: () => { if (canPull) runAction("GitPull", "Pulling changes…"); },
        p: () => { if (canPush) runAction("GitPush", "Pushing changes…"); },
        r: () => runAction("GitRefresh", "Fetching and refreshing…"),
        c: () => { if (tab === "changes" && canCommit) commitRef.current?.click(); },
      };
      const action = !hasShortcutModifier ? accelerators[key] : undefined;
      if (action) {
        event.preventDefault();
        event.stopImmediatePropagation();
        action();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [
    activeChangeIndex,
    canCommit,
    canPull,
    canPush,
    collapseCommit,
    copyLogHash,
    copyRawFile,
    expandCommit,
    expandedCommitIndex,
    expandedHash,
    focusChangeRow,
    focusLogItem,
    git.changes,
    gitLog,
    handleClose,
    logCommits,
    logFocus,
    moveLogFocus,
    requestLog,
    rollbackPaths,
    tab,
    toggleSelection,
    viewCommitDiff,
  ]);

  return (
    <div
      className="modal-overlay"
      role="dialog"
      aria-modal="true"
      onKeyDown={(event) => event.stopPropagation()}
      onKeyUp={(event) => event.stopPropagation()}
    >
      <ResizableModal
        ref={modalRef}
        modalId="git"
        client={client}
        className="git-modal"
        ariaLabel="Git repository"
        ariaModal
        defaultSize={{ width: 1400, height: 700 }}
        minSize={{ width: 760, height: 620 }}
        onPointerDownCapture={(event) => {
          if (rollbackPaths && !(event.target as Element).closest("[data-git-rollback]")) setRollbackPaths(null);
        }}
      >
        <div className="git-modal__header">
          <div>
            <span className="git-modal__eyebrow">REPOSITORY</span>
            <strong>{git.branch}</strong>
          </div>
          <button ref={closeRef} className="git-modal__close" title="Close (Esc)" onClick={handleClose}>×</button>
        </div>
        <div className="git-modal__summary" title={git.root}>
          <span className="git-modal__path">{git.root}</span>
          <span className="git-modal__stat">{git.stagedCount} staged</span>
          <span className="git-modal__stat">{git.modifiedCount} modified</span>
          <span className="git-modal__stat">{git.untrackedCount} untracked</span>
        </div>
        {!git.clean && (
          <div className="git-modal__warning">
            Staged changes are present. Branch switch and pull are disabled; pushing commits remains available.
          </div>
        )}
        <div className="git-modal__toolbar">
          <select
            ref={branchRef}
            value={git.branch}
            disabled={!git.clean}
            onChange={e => runAction("GitSwitchBranch", "Switching branch…", { branch: e.target.value })}
          >
            {git.branches.map(branch => <option key={branch} value={branch}>{branch}</option>)}
          </select>
          <kbd>B</kbd>
          <span className="git-modal__toolbar-spacer" />
          <button disabled={!canPull} title={!canPull ? "No incoming commits to pull" : undefined} onClick={() => runAction("GitPull", "Pulling changes…")}>
            Pull <kbd>L</kbd>
            {git.behindCount !== undefined && git.behindCount > 0 && (
              <em className="git-actions-modal__badge git-actions-modal__badge--incoming"><IconArrowDown size={13} /> {git.behindCount}</em>
            )}
          </button>
          <button disabled={!canPush} title={!canPush ? "No outgoing commits to push" : undefined} onClick={() => runAction("GitPush", "Pushing changes…")}>
            Push <kbd>P</kbd>
            {git.aheadCount !== undefined && git.aheadCount > 0 && (
              <em className="git-actions-modal__badge git-actions-modal__badge--outgoing"><IconArrowUp size={13} /> {git.aheadCount}</em>
            )}
          </button>
          <button onClick={() => runAction("GitRefresh", "Fetching and refreshing…")} title="Fetch remotes and refresh status">Refresh <kbd>R</kbd></button>
        </div>

        <div className="git-modal__tabs" role="tablist" aria-label="Git views">
          <button
            type="button"
            role="tab"
            aria-selected={tab === "changes"}
            data-testid="git-tab-changes"
            className={`git-modal__tab${tab === "changes" ? " git-modal__tab--active" : ""}`}
            onClick={() => setTab("changes")}
          >
            Changes <kbd>1</kbd>
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "log"}
            data-testid="git-tab-log"
            className={`git-modal__tab${tab === "log" ? " git-modal__tab--active" : ""}`}
            onClick={() => setTab("log")}
          >
            Log <kbd>2</kbd>
          </button>
        </div>

        {tab === "changes" ? (
          <>
            <div className="git-modal__section-title">
              <span>Changes</span>
              <label className="git-modal__select-all">
                <input
                  ref={selectAllRef}
                  type="checkbox"
                  data-testid="git-select-all-toggle"
                  aria-label="Select all Git changes"
                  checked={allChangesSelected}
                  disabled={git.changes.length === 0}
                  onChange={toggleAllChanges}
                />
                Select all
              </label>
              <span>{git.changes.length}</span>
            </div>
            {selectedChanges.length > 0 && (
              <div className="git-modal__selection-toolbar" aria-label={`${selectedChanges.length} selected Git changes`}>
                <strong data-testid="git-selection-count">{selectedChanges.length} selected</strong>
                <button data-testid="git-clear-selection" onClick={() => { setSelectedPaths(new Set()); selectionAnchorRef.current = null; setRollbackPaths(null); }}>Clear</button>
                <span />
                <button
                  data-testid="git-stage-selected"
                  disabled={selectedUnstaged.length === 0}
                  onClick={() => runAction("GitStagePaths", `Staging ${describePaths(selectedUnstaged.map(change => change.path))}…`, { paths: selectedUnstaged.map(change => change.path) })}
                >
                  Stage {selectedUnstaged.length}
                </button>
                <button
                  data-testid="git-unstage-selected"
                  disabled={selectedStaged.length === 0}
                  onClick={() => runAction("GitUnstagePaths", `Unstaging ${describePaths(selectedStaged.map(change => change.path))}…`, { paths: selectedStaged.map(change => change.path) })}
                >
                  Unstage {selectedStaged.length}
                </button>
                <button
                  data-git-rollback
                  data-testid="git-rollback-selected"
                  className={`git-modal__change-action git-modal__change-action--danger${samePathSet(rollbackPaths, selectedChanges.map(change => change.path)) ? " git-modal__change-action--confirm" : ""}`}
                  onClick={() => armRollback(selectedChanges.map(change => change.path))}
                >
                  {samePathSet(rollbackPaths, selectedChanges.map(change => change.path))
                    ? `Confirm rollback ${selectedChanges.length}`
                    : `Rollback ${selectedChanges.length}`}
                </button>
              </div>
            )}
            <div className="git-modal__changes" role="tabpanel">
              {git.changes.map((change, index) => {
                const stagedChange = isStaged(change);
                const actionLabel = stagedChange ? "Unstage" : "Stage";
                const rollbackPending = rollbackPaths?.length === 1 && rollbackPaths[0] === change.path;
                return (
                  <div
                    ref={(element) => { changeRowRefs.current[index] = element; }}
                    className={`git-modal__change${index === activeChangeIndex ? " git-modal__change--active" : ""}${selectedPaths.has(change.path) ? " git-modal__change--selected" : ""}`}
                    key={change.path}
                    role="group"
                    tabIndex={0}
                    aria-label={`Git change ${index + 1} of ${git.changes.length}: ${change.path}`}
                    aria-keyshortcuts="Space"
                    data-testid="git-change-row"
                    data-change-index={index}
                    onFocus={() => setActiveChangeIndex(index)}
                  >
                    <input
                      type="checkbox"
                      data-testid="git-change-select"
                      aria-label={`Select ${change.path}`}
                      checked={selectedPaths.has(change.path)}
                      onClick={event => { event.stopPropagation(); toggleSelection(index, event.shiftKey); }}
                      onChange={() => undefined}
                    />
                    <code className={stagedChange ? "git-modal__change-state--staged" : ""}>{change.indexStatus}{change.worktreeStatus}</code>
                    <span title={change.path}>{change.path}</span>
                    <button
                      className="git-modal__change-action"
                      data-testid="git-view-diff"
                      title="View diff (V)"
                      aria-keyshortcuts="v"
                      onFocus={() => setActiveChangeIndex(index)}
                      onClick={() => { setRollbackPaths(null); act("GitViewDiff", { targetPath: change.path }); }}
                    >
                      <IconGitCompare size={15} aria-hidden />View diff <kbd>V</kbd>
                    </button>
                    <button
                      className="git-modal__change-action"
                      title={`${actionLabel} (S)`}
                      aria-keyshortcuts="s"
                      onFocus={() => setActiveChangeIndex(index)}
                      onClick={() => {
                        setRollbackPaths(null);
                        runAction(
                          stagedChange ? "GitUnstagePaths" : "GitStagePaths",
                          `${stagedChange ? "Unstaging" : "Staging"} ${change.path}…`,
                          { paths: [change.path] },
                        );
                      }}
                    >
                      {stagedChange ? <IconMinus size={15} aria-hidden /> : <IconPlus size={15} aria-hidden />}
                      {actionLabel} <kbd>S</kbd>
                    </button>
                    <button
                      data-git-rollback
                      data-testid="git-rollback-path"
                      className={`git-modal__change-action git-modal__change-action--danger${rollbackPending ? " git-modal__change-action--confirm" : ""}`}
                      onFocus={() => setActiveChangeIndex(index)}
                      onClick={() => armRollback([change.path])}
                    >
                      {rollbackPending ? "Confirm rollback" : "Rollback"}
                    </button>
                  </div>
                );
              })}
            </div>
            <div className="git-modal__commit">
              <label htmlFor="git-commit-message">Commit message <span>{staged.length} staged</span></label>
              <textarea
                id="git-commit-message"
                value={message}
                onChange={e => setMessage(e.target.value)}
                placeholder="Describe this change"
                rows={2}
              />
            </div>
          </>
        ) : (
          <>
            <div className="git-modal__section-title">
              <span>Log</span>
              <span>{gitLog?.status === "ready" ? logCommits.length : "…"}</span>
            </div>
            <div className="git-modal__log" role="tabpanel" data-testid="git-log-panel">
              {gitLog?.status === "loading" && logCommits.length === 0 && (
                <div className="git-modal__log-empty">
                  <IconLoader2 size={16} className="git-modal__log-spinner" /> Loading history…
                </div>
              )}
              {gitLog?.status === "error" && (
                <div className="git-modal__log-empty git-modal__log-empty--error">{gitLog.error ?? "Could not load git log."}</div>
              )}
              {gitLog?.status === "ready" && logCommits.length === 0 && (
                <div className="git-modal__log-empty">No commits yet.</div>
              )}
              {logCommits.map((commit, index) => {
                const isExpanded = expandedHash === commit.hash;
                const show = gitLog?.commitShows[commit.hash];
                const commitActive = logFocus.kind === "commit" && logFocus.index === index;
                return (
                  <div key={commit.hash} className={`git-modal__log-entry${isExpanded ? " git-modal__log-entry--expanded" : ""}`}>
                    <div
                      ref={(element) => { logRowRefs.current[index] = element; }}
                      className={`git-modal__log-row${commitActive ? " git-modal__log-row--active" : ""}`}
                      role="group"
                      tabIndex={0}
                      aria-expanded={isExpanded}
                      aria-label={`Commit ${index + 1} of ${logCommits.length}: ${commit.shortHash} ${commit.subject}`}
                      data-testid="git-log-row"
                      data-log-index={index}
                      onFocus={() => setLogFocus({ kind: "commit", index })}
                      onDoubleClick={() => {
                        if (isExpanded) collapseCommit();
                        else expandCommit(commit.hash);
                      }}
                    >
                      <span className="git-modal__log-hash-cell">
                        <button
                          type="button"
                          className="git-modal__log-copy"
                          title="Copy full hash (Y)"
                          aria-keyshortcuts="y"
                          onFocus={() => setLogFocus({ kind: "commit", index })}
                          onClick={() => copyLogHash(commit.hash)}
                        >
                          <IconCopy size={14} aria-hidden />
                        </button>
                        <code className="git-modal__log-hash" title={commit.hash}>{commit.shortHash}</code>
                      </span>
                      <span className={`git-modal__log-subject-cell${commit.refs && commit.refs.length > 0 ? " git-modal__log-subject-cell--has-refs" : ""}`}>
                        <span className="git-modal__log-subject" title={commit.subject}>{commit.subject}</span>
                        {commit.refs && commit.refs.length > 0 && (
                          <LogRefsStrip refs={commit.refs} />
                        )}
                      </span>
                      <span className="git-modal__log-author" title={commit.authorEmail || commit.authorName}>{commit.authorName}</span>
                      <span className="git-modal__log-date" title={commit.authorDate}>{formatLogDate(commit.authorDate)}</span>
                    </div>
                    {isExpanded && (
                      <div className="git-modal__log-show" data-testid="git-log-commit-show" aria-label={`Changes in ${commit.shortHash}`}>
                        {show?.status === "loading" && (
                          <div className="git-modal__log-show-status">
                            <IconLoader2 size={14} className="git-modal__log-spinner" /> Loading changes…
                          </div>
                        )}
                        {show?.status === "error" && (
                          <div className="git-modal__log-show-status git-modal__log-show-status--error">
                            {show.error ?? "Could not load commit changes."}
                          </div>
                        )}
                        {show?.status === "ready" && show.changes.length === 0 && (
                          <div className="git-modal__log-show-status">No file changes.</div>
                        )}
                        {show?.status === "ready" && show.changes.map((change, changeIndex) => {
                          const changeActive = logFocus.kind === "change"
                            && logFocus.commitIndex === index
                            && logFocus.changeIndex === changeIndex;
                          const pathLabel = change.oldPath
                            ? `${change.oldPath} → ${change.path}`
                            : change.path;
                          return (
                            <div
                              key={`${change.path}:${changeIndex}`}
                              ref={(element) => {
                                if (!logChangeRowRefs.current[commit.hash]) {
                                  logChangeRowRefs.current[commit.hash] = [];
                                }
                                logChangeRowRefs.current[commit.hash][changeIndex] = element;
                              }}
                              className={`git-modal__log-change${changeActive ? " git-modal__log-change--active" : ""}`}
                              role="group"
                              tabIndex={0}
                              aria-label={`Change ${changeIndex + 1} of ${show.changes.length}: ${pathLabel}`}
                              data-testid="git-log-change-row"
                              data-change-index={changeIndex}
                              onFocus={() => setLogFocus({ kind: "change", commitIndex: index, changeIndex })}
                            >
                              <code className="git-modal__log-change-state">{change.indexStatus}</code>
                              <span className="git-modal__log-change-path" title={pathLabel}>{pathLabel}</span>
                              <button
                                type="button"
                                className="git-modal__change-action"
                                data-testid="git-log-view-diff"
                                title="View diff (V)"
                                aria-keyshortcuts="v"
                                onFocus={() => setLogFocus({ kind: "change", commitIndex: index, changeIndex })}
                                onClick={() => viewCommitDiff(commit.hash, change)}
                              >
                                <IconGitCompare size={15} aria-hidden />View diff <kbd>V</kbd>
                              </button>
                              {change.indexStatus !== "D" && (
                                <button
                                  type="button"
                                  className="git-modal__change-action"
                                  data-testid="git-log-copy-raw"
                                  title="Copy raw file reference (C) — paste with Ctrl+V"
                                  aria-keyshortcuts="c"
                                  onFocus={() => setLogFocus({ kind: "change", commitIndex: index, changeIndex })}
                                  onClick={() => copyRawFile(commit.hash, change)}
                                >
                                  <IconCopy size={15} aria-hidden />Copy raw <kbd>C</kbd>
                                </button>
                              )}
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
              {gitLog?.hasMore && (
                <div className="git-modal__log-more" ref={logLoadMoreRef}>
                  <button
                    type="button"
                    className="git-modal__change-action"
                    data-testid="git-log-load-more"
                    disabled={gitLog.status === "loading"}
                    onClick={() => requestLog({ skip: logCommits.length, append: true })}
                    title="Load more (Space)"
                  >
                    {gitLog.status === "loading" ? "Loading…" : <>Load more <kbd>SPACE</kbd></>}
                  </button>
                </div>
              )}
            </div>
          </>
        )}

        <div className="modal__actions">
          {activity && (
            <span className="git-modal__activity">
              {activity.action !== "complete" && <IconLoader2 size={14} />}
              {activity.message}
            </span>
          )}
          <button onClick={handleClose}>Close</button>
          {tab === "changes" && (
            <button
              ref={commitRef}
              disabled={!canCommit}
              onClick={() => runAction("GitCommit", "Committing staged changes…", { message })}
            >
              Commit staged <kbd>C</kbd>
            </button>
          )}
        </div>
      </ResizableModal>
    </div>
  );
}

function isStaged(change: GitRepoStatus["changes"][number]) {
  return change.indexStatus !== " " && change.indexStatus !== "?";
}

function samePathSet(left: Iterable<string> | null, right: Iterable<string>) {
  if (!left) return false;
  const leftSet = new Set(left);
  const rightSet = new Set(right);
  return leftSet.size === rightSet.size && [...leftSet].every(path => rightSet.has(path));
}

function describePaths(paths: string[]) {
  return paths.length === 1 ? paths[0] : `${paths.length} files`;
}

function formatLogDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const p = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** Horizontally pannable ref chips. overflow:hidden — no native scrollbar. */
function LogRefsStrip({ refs }: { refs: GitLogRef[] }) {
  const wrapRef = useRef<HTMLSpanElement>(null);
  const scrollerRef = useRef<HTMLSpanElement>(null);
  // Prefer trailing chips (right-aligned look). Stay pinned to the end until the user pans.
  const pinToEndRef = useRef(true);
  const [fadeLeft, setFadeLeft] = useState(false);
  const [fadeRight, setFadeRight] = useState(false);

  const updateFades = useCallback(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const maxScroll = Math.max(0, el.scrollWidth - el.clientWidth);
    const isOverflowing = maxScroll > 1;
    // Pin to trailing edge so the rightmost chips stay visible (matches prior flex-end look).
    if (pinToEndRef.current && isOverflowing) {
      el.scrollLeft = maxScroll;
    }
    const { scrollLeft } = el;
    pinToEndRef.current = !isOverflowing || scrollLeft >= maxScroll - 1;
    setFadeLeft(scrollLeft > 1);
    setFadeRight(isOverflowing && scrollLeft < maxScroll - 1);
  }, []);

  // Reset pin when the ref set changes so new chips re-align to the trailing edge.
  useEffect(() => {
    pinToEndRef.current = true;
  }, [refs]);

  // While the pointer is over the chip strip, keep wheel events here: map
  // vertical scroll to horizontal chip pan and never bubble to the log list.
  // scrollLeft works with overflow:hidden (clips without painting a scrollbar).
  // Edge fades via mask-image track overflow + scroll position.
  useEffect(() => {
    const wrap = wrapRef.current;
    const el = scrollerRef.current;
    if (!wrap || !el) return;
    const onWheel = (event: WheelEvent) => {
      if (el.scrollWidth <= el.clientWidth) return;
      event.preventDefault();
      event.stopPropagation();
      const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
      if (delta !== 0) {
        pinToEndRef.current = false;
        el.scrollLeft += delta;
        updateFades();
      }
    };
    wrap.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("scroll", updateFades, { passive: true });
    const ro = new ResizeObserver(() => updateFades());
    ro.observe(el);
    ro.observe(wrap);
    const id = requestAnimationFrame(() => updateFades());
    return () => {
      wrap.removeEventListener("wheel", onWheel);
      el.removeEventListener("scroll", updateFades);
      ro.disconnect();
      cancelAnimationFrame(id);
    };
  }, [refs, updateFades]);

  const wrapClass = [
    "git-modal__log-refs-wrap",
    fadeLeft ? "git-modal__log-refs-wrap--fade-left" : "",
    fadeRight ? "git-modal__log-refs-wrap--fade-right" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <span
      ref={wrapRef}
      className={wrapClass}
      title={refs.map((ref) => ref.name).join(", ")}
      onClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => event.stopPropagation()}
    >
      <span className="git-modal__log-refs" ref={scrollerRef}>
        {/* Grows when chips fit (right-aligns them); collapses to 0 when overflowing. */}
        <span className="git-modal__log-refs-spacer" aria-hidden="true" />
        {refs.map((ref) => (
          <span
            key={`${ref.kind}:${ref.name}`}
            className={`git-modal__log-ref git-modal__log-ref--${ref.kind}`}
          >
            {ref.name}
          </span>
        ))}
      </span>
    </span>
  );
}
