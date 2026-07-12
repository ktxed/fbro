import { useEffect, useRef } from "react";
import { IconArrowDown, IconArrowUp, IconGitBranch, IconRefresh } from "@tabler/icons-react";
import type { GitRepoStatus } from "../ws/types";
import type { WsClient } from "../ws/client";

interface Props {
  panelId: string;
  git: GitRepoStatus;
  client: WsClient;
  onClose: () => void;
}

export function GitActionsModal({ panelId, git, client, onClose }: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const act = (name: string, args: Record<string, unknown> = {}) => {
    client.sendIntent(name, undefined, { panelId, ...args });
  };

  useEffect(() => {
    dialogRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [onClose]);

  const disabledMessage = "Requires a clean worktree";
  return (
    <div className="modal-overlay" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div ref={dialogRef} className="git-actions-modal" role="dialog" aria-modal="true" aria-label="Git actions" tabIndex={-1} onKeyDown={(event) => event.stopPropagation()}>
        <header className="git-actions-modal__header">
          <div><span>GIT</span><strong><IconGitBranch size={17} /> {git.branch}</strong></div>
          <button type="button" className="git-actions-modal__close" onClick={onClose} title="Close">×</button>
        </header>
        <div className="git-actions-modal__repo" title={git.root}>{git.root}</div>
        {!git.clean && <div className="git-actions-modal__warning">Working tree has changes. Switch, pull, and push require a clean worktree.</div>}
        <div className="git-actions-modal__choices">
          <button type="button" className="git-actions-modal__choice" onClick={() => act("GitFetch")}>
            <IconRefresh size={19} /><span><strong>Fetch</strong><small>Update remote branch information</small></span>
          </button>
          <label className={`git-actions-modal__choice git-actions-modal__choice--branch${!git.clean ? " is-disabled" : ""}`} title={!git.clean ? disabledMessage : undefined}>
            <IconGitBranch size={19} /><span><strong>Switch branch</strong><small>Requires a clean worktree</small></span>
            <select value={git.branch} disabled={!git.clean} onChange={(event) => act("GitSwitchBranch", { branch: event.target.value })}>
              {git.branches.map((branch) => <option key={branch} value={branch}>{branch}</option>)}
            </select>
          </label>
          <button type="button" className="git-actions-modal__choice" disabled={!git.clean} title={!git.clean ? disabledMessage : undefined} onClick={() => act("GitPull")}>
            <IconArrowDown size={19} /><span><strong>Pull</strong><small>Bring in remote commits</small></span>
            {git.behindCount !== undefined && <em className="git-actions-modal__badge git-actions-modal__badge--incoming"><IconArrowDown size={13} /> {git.behindCount}</em>}
          </button>
          <button type="button" className="git-actions-modal__choice" disabled={!git.clean} title={!git.clean ? disabledMessage : undefined} onClick={() => act("GitPush")}>
            <IconArrowUp size={19} /><span><strong>Push</strong><small>Send local commits to remote</small></span>
            {git.aheadCount !== undefined && <em className="git-actions-modal__badge git-actions-modal__badge--outgoing"><IconArrowUp size={13} /> {git.aheadCount}</em>}
          </button>
        </div>
        <footer><span>{git.upstream ? `Tracking ${git.upstream}` : "No upstream configured"}</span><button type="button" onClick={onClose}>Close</button></footer>
      </div>
    </div>
  );
}
