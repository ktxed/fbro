import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { IconArrowDown, IconArrowUp, IconGitCompare, IconLoader2, IconMinus, IconPlus } from "@tabler/icons-react";
import type { GitRepoStatus } from "../ws/types";
import type { WsClient } from "../ws/client";
import { ResizableModal } from "./ResizableModal";

export function GitModal({ panelId, git, client, onClose }: { panelId: string; git: GitRepoStatus; client: WsClient; onClose: () => void }) {
  const [message, setMessage] = useState("");
  const staged = useMemo(() => git.changes.filter(c => c.indexStatus !== " " && c.indexStatus !== "?"), [git]);
  const modalRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const branchRef = useRef<HTMLSelectElement>(null);
  const commitRef = useRef<HTMLButtonElement>(null);
  const selectAllRef = useRef<HTMLInputElement>(null);
  const changeRowRefs = useRef<Array<HTMLDivElement | null>>([]);
  const lastGitRef = useRef(git);
  const [activity, setActivity] = useState<{ action: string; message: string } | null>(null);
  const [activeChangeIndex, setActiveChangeIndex] = useState(0);
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
  useEffect(() => {
    runAction("GitRefresh", "Refreshing Git status…");
  }, [client, panelId]);
  useEffect(() => {
    if (activity && git !== lastGitRef.current) {
      setActivity({ action: "complete", message: `${activity.message.replace(/…$/, "")} complete` });
    }
    lastGitRef.current = git;
  }, [activity, git]);
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
        else onClose();
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
        const movement = event.key === "ArrowUp" || key === "k" ? -1 : event.key === "ArrowDown" || key === "j" ? 1 : 0;
        if (movement) {
          event.preventDefault();
          event.stopImmediatePropagation();
          focusChangeRow(activeChangeIndex + movement);
          return;
        }
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
      const accelerators: Record<string, () => void> = {
        b: () => branchRef.current?.focus(),
        l: () => { if (canPull) runAction("GitPull", "Pulling changes…"); },
        p: () => { if (canPush) runAction("GitPush", "Pushing changes…"); },
        r: () => runAction("GitRefresh", "Refreshing Git status…"),
        c: () => { if (canCommit) commitRef.current?.click(); },
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
  }, [activeChangeIndex, canCommit, canPull, canPush, focusChangeRow, git.changes, onClose, rollbackPaths, toggleSelection]);
  return <div className="modal-overlay" role="dialog" aria-modal="true" onKeyDown={(event) => event.stopPropagation()} onKeyUp={(event) => event.stopPropagation()}><ResizableModal ref={modalRef} modalId="git" client={client} className="git-modal" ariaLabel="Git repository" ariaModal defaultSize={{ width: 1400, height: 700 }} minSize={{ width: 760, height: 620 }} onPointerDownCapture={(event) => { if (rollbackPaths && !(event.target as Element).closest("[data-git-rollback]")) setRollbackPaths(null); }}>
    <div className="git-modal__header"><div><span className="git-modal__eyebrow">REPOSITORY</span><strong>{git.branch}</strong></div><button ref={closeRef} className="git-modal__close" title="Close (Esc)" onClick={onClose}>×</button></div>
    <div className="git-modal__summary" title={git.root}><span className="git-modal__path">{git.root}</span><span className="git-modal__stat">{git.stagedCount} staged</span><span className="git-modal__stat">{git.modifiedCount} modified</span><span className="git-modal__stat">{git.untrackedCount} untracked</span></div>
    {!git.clean && <div className="git-modal__warning">Staged changes are present. Branch switch and pull are disabled; pushing commits remains available.</div>}
    <div className="git-modal__toolbar"><select ref={branchRef} value={git.branch} disabled={!git.clean} onChange={e => runAction("GitSwitchBranch", "Switching branch…", { branch: e.target.value })}>{git.branches.map(branch => <option key={branch} value={branch}>{branch}</option>)}</select><kbd>B</kbd><span className="git-modal__toolbar-spacer" /><button disabled={!canPull} title={!canPull ? "No incoming commits to pull" : undefined} onClick={() => runAction("GitPull", "Pulling changes…")}>Pull <kbd>L</kbd>{git.behindCount !== undefined && git.behindCount > 0 && <em className="git-actions-modal__badge git-actions-modal__badge--incoming"><IconArrowDown size={13} /> {git.behindCount}</em>}</button><button disabled={!canPush} title={!canPush ? "No outgoing commits to push" : undefined} onClick={() => runAction("GitPush", "Pushing changes…")}>Push <kbd>P</kbd>{git.aheadCount !== undefined && git.aheadCount > 0 && <em className="git-actions-modal__badge git-actions-modal__badge--outgoing"><IconArrowUp size={13} /> {git.aheadCount}</em>}</button><button onClick={() => runAction("GitRefresh", "Refreshing Git status…")}>Refresh <kbd>R</kbd></button></div>
    <div className="git-modal__section-title"><span>Changes</span><label className="git-modal__select-all"><input ref={selectAllRef} type="checkbox" data-testid="git-select-all-toggle" aria-label="Select all Git changes" checked={allChangesSelected} disabled={git.changes.length === 0} onChange={toggleAllChanges} />Select all</label><span>{git.changes.length}</span></div>
    {selectedChanges.length > 0 && <div className="git-modal__selection-toolbar" aria-label={`${selectedChanges.length} selected Git changes`}><strong data-testid="git-selection-count">{selectedChanges.length} selected</strong><button data-testid="git-clear-selection" onClick={() => { setSelectedPaths(new Set()); selectionAnchorRef.current = null; setRollbackPaths(null); }}>Clear</button><span /><button data-testid="git-stage-selected" disabled={selectedUnstaged.length === 0} onClick={() => runAction("GitStagePaths", `Staging ${describePaths(selectedUnstaged.map(change => change.path))}…`, { paths: selectedUnstaged.map(change => change.path) })}>Stage {selectedUnstaged.length}</button><button data-testid="git-unstage-selected" disabled={selectedStaged.length === 0} onClick={() => runAction("GitUnstagePaths", `Unstaging ${describePaths(selectedStaged.map(change => change.path))}…`, { paths: selectedStaged.map(change => change.path) })}>Unstage {selectedStaged.length}</button><button data-git-rollback data-testid="git-rollback-selected" className={`git-modal__change-action git-modal__change-action--danger${samePathSet(rollbackPaths, selectedChanges.map(change => change.path)) ? " git-modal__change-action--confirm" : ""}`} onClick={() => armRollback(selectedChanges.map(change => change.path))}>{samePathSet(rollbackPaths, selectedChanges.map(change => change.path)) ? `Confirm rollback ${selectedChanges.length}` : `Rollback ${selectedChanges.length}`}</button></div>}
    <div className="git-modal__changes">{git.changes.map((change, index) => { const stagedChange = isStaged(change); const actionLabel = stagedChange ? "Unstage" : "Stage"; const rollbackPending = rollbackPaths?.length === 1 && rollbackPaths[0] === change.path; return <div ref={(element) => { changeRowRefs.current[index] = element; }} className={`git-modal__change${index === activeChangeIndex ? " git-modal__change--active" : ""}${selectedPaths.has(change.path) ? " git-modal__change--selected" : ""}`} key={change.path} role="group" tabIndex={0} aria-label={`Git change ${index + 1} of ${git.changes.length}: ${change.path}`} aria-keyshortcuts="Space" data-testid="git-change-row" data-change-index={index} onFocus={() => setActiveChangeIndex(index)}><input type="checkbox" data-testid="git-change-select" aria-label={`Select ${change.path}`} checked={selectedPaths.has(change.path)} onClick={event => { event.stopPropagation(); toggleSelection(index, event.shiftKey); }} onChange={() => undefined} /><code className={stagedChange ? "git-modal__change-state--staged" : ""}>{change.indexStatus}{change.worktreeStatus}</code><span title={change.path}>{change.path}</span><button className="git-modal__change-action" data-testid="git-view-diff" title="View diff (V)" aria-keyshortcuts="v" onFocus={() => setActiveChangeIndex(index)} onClick={() => { setRollbackPaths(null); act("GitViewDiff", { targetPath: change.path }); }}><IconGitCompare size={15} aria-hidden />View diff <kbd>V</kbd></button><button className="git-modal__change-action" title={`${actionLabel} (S)`} aria-keyshortcuts="s" onFocus={() => setActiveChangeIndex(index)} onClick={() => { setRollbackPaths(null); runAction(stagedChange ? "GitUnstagePaths" : "GitStagePaths", `${stagedChange ? "Unstaging" : "Staging"} ${change.path}…`, { paths: [change.path] }); }}>{stagedChange ? <IconMinus size={15} aria-hidden /> : <IconPlus size={15} aria-hidden />}{actionLabel} <kbd>S</kbd></button><button data-git-rollback data-testid="git-rollback-path" className={`git-modal__change-action git-modal__change-action--danger${rollbackPending ? " git-modal__change-action--confirm" : ""}`} onFocus={() => setActiveChangeIndex(index)} onClick={() => armRollback([change.path])}>{rollbackPending ? "Confirm rollback" : "Rollback"}</button></div>; })}</div>
    <div className="git-modal__commit"><label htmlFor="git-commit-message">Commit message <span>{staged.length} staged</span></label><textarea id="git-commit-message" value={message} onChange={e => setMessage(e.target.value)} placeholder="Describe this change" rows={2} /></div>
    <div className="modal__actions">{activity && <span className="git-modal__activity">{activity.action !== "complete" && <IconLoader2 size={14} />}{activity.message}</span>}<button onClick={onClose}>Close</button><button ref={commitRef} disabled={!canCommit} onClick={() => runAction("GitCommit", "Committing staged changes…", { message })}>Commit staged <kbd>C</kbd></button></div>
  </ResizableModal></div>;
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
