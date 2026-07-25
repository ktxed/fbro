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
  const changeRowRefs = useRef<Array<HTMLDivElement | null>>([]);
  const lastGitRef = useRef(git);
  const [activity, setActivity] = useState<{ action: string; message: string } | null>(null);
  const [activeChangeIndex, setActiveChangeIndex] = useState(0);
  const canCommit = message.trim().length > 0 && staged.length > 0;
  const canPull = git.clean && git.behindCount !== undefined && git.behindCount > 0;
  const canPush = !!git.upstream && git.aheadCount !== undefined && git.aheadCount > 0;
  const act = (name: string, args: Record<string, unknown> = {}) => client.sendIntent(name, undefined, { panelId, ...args });
  const runAction = (name: string, message: string, args: Record<string, unknown> = {}) => {
    setActivity({ action: name, message });
    act(name, args);
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
  }, [git.changes.length]);
  useEffect(() => {
    const frame = requestAnimationFrame(() => closeRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, []);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        onClose();
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
  }, [activeChangeIndex, canCommit, canPull, canPush, focusChangeRow, git.changes, onClose]);
  return <div className="modal-overlay" role="dialog" aria-modal="true" onKeyDown={(event) => event.stopPropagation()} onKeyUp={(event) => event.stopPropagation()}><ResizableModal ref={modalRef} modalId="git" client={client} className="git-modal" ariaLabel="Git repository" ariaModal defaultSize={{ width: 1400, height: 700 }} minSize={{ width: 760, height: 620 }}>
    <div className="git-modal__header"><div><span className="git-modal__eyebrow">REPOSITORY</span><strong>{git.branch}</strong></div><button ref={closeRef} className="git-modal__close" title="Close (Esc)" onClick={onClose}>×</button></div>
    <div className="git-modal__summary" title={git.root}><span className="git-modal__path">{git.root}</span><span className="git-modal__stat">{git.stagedCount} staged</span><span className="git-modal__stat">{git.modifiedCount} modified</span><span className="git-modal__stat">{git.untrackedCount} untracked</span></div>
    {!git.clean && <div className="git-modal__warning">Staged changes are present. Branch switch and pull are disabled; pushing commits remains available.</div>}
    <div className="git-modal__toolbar"><select ref={branchRef} value={git.branch} disabled={!git.clean} onChange={e => runAction("GitSwitchBranch", "Switching branch…", { branch: e.target.value })}>{git.branches.map(branch => <option key={branch} value={branch}>{branch}</option>)}</select><kbd>B</kbd><span className="git-modal__toolbar-spacer" /><button disabled={!canPull} title={!canPull ? "No incoming commits to pull" : undefined} onClick={() => runAction("GitPull", "Pulling changes…")}>Pull <kbd>L</kbd>{git.behindCount !== undefined && git.behindCount > 0 && <em className="git-actions-modal__badge git-actions-modal__badge--incoming"><IconArrowDown size={13} /> {git.behindCount}</em>}</button><button disabled={!canPush} title={!canPush ? "No outgoing commits to push" : undefined} onClick={() => runAction("GitPush", "Pushing changes…")}>Push <kbd>P</kbd>{git.aheadCount !== undefined && git.aheadCount > 0 && <em className="git-actions-modal__badge git-actions-modal__badge--outgoing"><IconArrowUp size={13} /> {git.aheadCount}</em>}</button><button onClick={() => runAction("GitRefresh", "Refreshing Git status…")}>Refresh <kbd>R</kbd></button></div>
    <div className="git-modal__section-title">Changes <span>{git.changes.length}</span></div>
    <div className="git-modal__changes">{git.changes.map((change, index) => { const isStaged = change.indexStatus !== " " && change.indexStatus !== "?"; const actionLabel = isStaged ? "Unstage" : "Stage"; return <div ref={(element) => { changeRowRefs.current[index] = element; }} className={`git-modal__change${index === activeChangeIndex ? " git-modal__change--active" : ""}`} key={change.path} role="group" tabIndex={0} aria-label={`Git change ${index + 1} of ${git.changes.length}: ${change.path}`} data-testid="git-change-row" data-change-index={index} onFocus={() => setActiveChangeIndex(index)}><code className={isStaged ? "git-modal__change-state--staged" : ""}>{change.indexStatus}{change.worktreeStatus}</code><span title={change.path}>{change.path}</span><button className="git-modal__change-action" data-testid="git-view-diff" title="View diff (V)" aria-keyshortcuts="v" onFocus={() => setActiveChangeIndex(index)} onClick={() => act("GitViewDiff", { targetPath: change.path })}><IconGitCompare size={15} aria-hidden />View diff <kbd>V</kbd></button><button className="git-modal__change-action" title={`${actionLabel} (S)`} aria-keyshortcuts="s" onFocus={() => setActiveChangeIndex(index)} onClick={() => runAction(isStaged ? "GitUnstagePaths" : "GitStagePaths", `${isStaged ? "Unstaging" : "Staging"} ${change.path}…`, { paths: [change.path] })}>{isStaged ? <IconMinus size={15} aria-hidden /> : <IconPlus size={15} aria-hidden />}{actionLabel} <kbd>S</kbd></button></div>; })}</div>
    <div className="git-modal__commit"><label htmlFor="git-commit-message">Commit message <span>{staged.length} staged</span></label><textarea id="git-commit-message" value={message} onChange={e => setMessage(e.target.value)} placeholder="Describe this change" rows={2} /></div>
    <div className="modal__actions">{activity && <span className="git-modal__activity">{activity.action !== "complete" && <IconLoader2 size={14} />}{activity.message}</span>}<button onClick={onClose}>Close</button><button ref={commitRef} disabled={!canCommit} onClick={() => runAction("GitCommit", "Committing staged changes…", { message })}>Commit staged <kbd>C</kbd></button></div>
  </ResizableModal></div>;
}
