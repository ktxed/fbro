import { useCallback, useEffect, useRef, useState } from "react";
import { IconArrowDown, IconArrowUp, IconGitBranch, IconLoader2, IconRefresh } from "@tabler/icons-react";
import type { GitRepoStatus } from "../ws/types";
import type { WsClient } from "../ws/client";

interface Props {
  panelId: string;
  git: GitRepoStatus;
  client: WsClient;
  onClose: () => void;
  onOpenRepository: () => void;
}

export function GitActionsModal({ panelId, git, client, onClose, onOpenRepository }: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const controlRefs = useRef<Array<HTMLButtonElement | HTMLSelectElement | null>>([]);
  const lastGitRef = useRef(git);
  const [activeIndex, setActiveIndex] = useState(0);
  const [activity, setActivity] = useState<{ action: string; message: string } | null>(null);
  const canPull = git.clean && git.behindCount !== undefined && git.behindCount > 0;
  const canPush = git.clean && !!git.upstream && git.aheadCount !== undefined && git.aheadCount > 0;
  const pullDisabledMessage = !git.clean ? "Requires no staged changes" : !git.upstream ? "No upstream configured" : git.behindCount === 0 ? "Already up to date" : "Remote status unavailable — fetch first";
  const pushDisabledMessage = !git.clean ? "Requires no staged changes" : !git.upstream ? "No upstream configured — set an upstream first" : git.aheadCount === 0 ? "No outgoing commits" : "Remote status unavailable — fetch first";
  const act = (name: string, args: Record<string, unknown> = {}) => {
    client.sendIntent(name, undefined, { panelId, ...args });
  };
  const runAction = (name: string, message: string, args: Record<string, unknown> = {}) => {
    setActivity({ action: name, message });
    act(name, args);
  };

  useEffect(() => {
    runAction("GitRefresh", "Refreshing Git status…");
  }, [client, panelId]);

  useEffect(() => {
    if (activity && git !== lastGitRef.current) {
      setActivity({ action: "complete", message: `${activity.message.replace(/…$/, "")} complete` });
    }
    lastGitRef.current = git;
  }, [activity, git]);

  const focusControl = useCallback((index: number) => {
    const control = controlRefs.current[index];
    if (!control || (control instanceof HTMLButtonElement && control.disabled)) return;
    setActiveIndex(index);
    control.focus();
  }, []);

  useEffect(() => {
    dialogRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.shiftKey && event.key.toLowerCase() === "g") {
        event.preventDefault();
        event.stopPropagation();
        onOpenRepository();
        return;
      }
      const key = event.key.toLowerCase();
      const isBranchPicker = event.target instanceof HTMLSelectElement;
      // Textual branch selection remains native, but arrows always navigate
      // the action grid as advertised by this chooser.
      if (isBranchPicker && !event.key.startsWith("Arrow")) return;
      const accelerators: Record<string, () => void> = {
        f: () => runAction("GitFetch", "Fetching remote…"),
        b: () => focusControl(1),
        l: () => { if (canPull) runAction("GitPull", "Pulling changes…"); },
        p: () => { if (canPush) runAction("GitPush", "Pushing changes…"); },
        u: () => { if (!git.upstream && git.clean && !git.detached) runAction("GitSetUpstream", "Setting upstream…", { branch: git.branch }); },
      };
      if (accelerators[key]) {
        event.preventDefault();
        event.stopPropagation();
        accelerators[key]();
        return;
      }
      const offsets: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -2, ArrowDown: 2 };
      const offset = offsets[event.key];
      if (offset === undefined) return;
      const candidate = activeIndex + offset;
      if (candidate < 0 || candidate > 3) return;
      event.preventDefault();
      event.stopPropagation();
      focusControl(candidate);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [activeIndex, canPull, canPush, focusControl, git.branch, git.clean, git.detached, git.upstream, onClose, onOpenRepository]);

  const disabledMessage = "Requires no staged changes";
  return (
    <div className="modal-overlay" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div ref={dialogRef} className="git-actions-modal" role="dialog" aria-modal="true" aria-label="Git actions" tabIndex={-1} onKeyDown={(event) => event.stopPropagation()}>
        <header className="git-actions-modal__header">
          <div><span>GIT</span><strong><IconGitBranch size={17} /> {git.branch}</strong></div>
          <button type="button" className="git-actions-modal__close" onClick={onClose} title="Close">×</button>
        </header>
        <div className="git-actions-modal__repo" title={git.root}>{git.root}</div>
        {!git.clean && <div className="git-actions-modal__warning">Staged changes are present. Switch, pull, and push require no staged changes.</div>}
        <div className="git-actions-modal__choices">
          <button ref={(el) => { controlRefs.current[0] = el; }} type="button" className={`git-actions-modal__choice${activeIndex === 0 ? " is-keyboard-active" : ""}`} onFocus={() => setActiveIndex(0)} onClick={() => runAction("GitFetch", "Fetching remote…")}>
            <IconRefresh size={19} /><span><strong>Fetch <kbd>F</kbd></strong><small>Update remote branch information</small></span>
          </button>
          <label className={`git-actions-modal__choice git-actions-modal__choice--branch${!git.clean ? " is-disabled" : ""}${activeIndex === 1 ? " is-keyboard-active" : ""}`} title={!git.clean ? disabledMessage : undefined}>
            <IconGitBranch size={19} /><span><strong>Switch branch <kbd>B</kbd></strong><small>Requires no staged changes</small></span>
            <select ref={(el) => { controlRefs.current[1] = el; }} value={git.branch} disabled={!git.clean} onFocus={() => setActiveIndex(1)} onChange={(event) => runAction("GitSwitchBranch", "Switching branch…", { branch: event.target.value })}>
              {git.branches.map((branch) => <option key={branch} value={branch}>{branch}</option>)}
            </select>
          </label>
          <button ref={(el) => { controlRefs.current[2] = el; }} type="button" className={`git-actions-modal__choice${activeIndex === 2 ? " is-keyboard-active" : ""}`} disabled={!canPull} title={!canPull ? pullDisabledMessage : undefined} onFocus={() => setActiveIndex(2)} onClick={() => runAction("GitPull", "Pulling changes…")}>
            <IconArrowDown size={19} /><span><strong>Pull <kbd>L</kbd></strong><small>Bring in remote commits</small></span>
            {git.behindCount !== undefined && git.behindCount > 0 && <em className="git-actions-modal__badge git-actions-modal__badge--incoming"><IconArrowDown size={13} /> {git.behindCount}</em>}
          </button>
          <button ref={(el) => { controlRefs.current[3] = el; }} type="button" className={`git-actions-modal__choice${activeIndex === 3 ? " is-keyboard-active" : ""}`} disabled={!canPush} title={!canPush ? pushDisabledMessage : undefined} onFocus={() => setActiveIndex(3)} onClick={() => runAction("GitPush", "Pushing changes…")}>
            <IconArrowUp size={19} /><span><strong>Push <kbd>P</kbd></strong><small>Send local commits to remote</small></span>
            {git.aheadCount !== undefined && git.aheadCount > 0 && <em className="git-actions-modal__badge git-actions-modal__badge--outgoing"><IconArrowUp size={13} /> {git.aheadCount}</em>}
          </button>
        </div>
        <footer><span className={activity ? "git-actions-modal__activity" : undefined}>{activity && activity.action !== "complete" && <IconLoader2 size={14} />}{activity?.message ?? (git.upstream ? `Tracking ${git.upstream}` : "No upstream configured")}</span><div className="git-actions-modal__footer-actions">{!git.upstream && <button type="button" disabled={!git.clean || git.detached} title={!git.clean ? disabledMessage : git.detached ? "A detached HEAD cannot have a branch upstream" : "Push this branch to origin and configure its upstream"} onClick={() => runAction("GitSetUpstream", "Setting upstream…", { branch: git.branch })}>Set upstream <kbd>U</kbd></button>}<button type="button" onClick={onClose}>Close</button></div></footer>
      </div>
    </div>
  );
}
