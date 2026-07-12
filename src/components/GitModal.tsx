import { useEffect, useMemo, useState } from "react";
import type { GitRepoStatus } from "../ws/types";
import type { WsClient } from "../ws/client";
import { ResizableModal } from "./ResizableModal";

export function GitModal({ panelId, git, client, onClose }: { panelId: string; git: GitRepoStatus; client: WsClient; onClose: () => void }) {
  const [message, setMessage] = useState("");
  const staged = useMemo(() => git.changes.filter(c => c.indexStatus !== " " && c.indexStatus !== "?"), [git]);
  const act = (name: string, args: Record<string, unknown> = {}) => client.sendIntent(name, undefined, { panelId, ...args });
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        onClose();
        return;
      }
      // The panel keeps focus behind this modeless-looking overlay. Treat the
      // dialog as a real modal boundary so its shortcuts cannot leak into the
      // file list (for example ArrowUp navigating the underlying panel).
      const target = event.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return;
      event.preventDefault();
      event.stopImmediatePropagation();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [onClose]);
  return <div className="modal-overlay" role="dialog" aria-modal="true" onKeyDown={(event) => event.stopPropagation()} onKeyUp={(event) => event.stopPropagation()}><ResizableModal modalId="git" client={client} className="git-modal" ariaLabel="Git repository" ariaModal defaultSize={{ width: 760, height: 700 }} minSize={{ width: 640, height: 620 }}>
    <div className="git-modal__header"><div><span className="git-modal__eyebrow">REPOSITORY</span><strong>{git.branch}</strong></div><button className="git-modal__close" title="Close" onClick={onClose}>×</button></div>
    <div className="git-modal__summary" title={git.root}><span className="git-modal__path">{git.root}</span><span className="git-modal__stat">{git.stagedCount} staged</span><span className="git-modal__stat">{git.modifiedCount} modified</span><span className="git-modal__stat">{git.untrackedCount} untracked</span></div>
    {!git.clean && <div className="git-modal__warning">Working tree has changes. Branch switch, pull, and push are disabled; use another Git tool to stash or resolve them.</div>}
    <div className="git-modal__toolbar"><select value={git.branch} disabled={!git.clean} onChange={e => act("GitSwitchBranch", { branch: e.target.value })}>{git.branches.map(branch => <option key={branch} value={branch}>{branch}</option>)}</select><span className="git-modal__toolbar-spacer" /><button disabled={!git.clean} onClick={() => act("GitPull")}>Pull</button><button disabled={!git.clean} onClick={() => act("GitPush")}>Push</button><button onClick={() => act("GitRefresh")}>Refresh</button></div>
    <div className="git-modal__section-title">Changes <span>{git.changes.length}</span></div>
    <div className="git-modal__changes">{git.changes.map(change => { const isStaged = change.indexStatus !== " " && change.indexStatus !== "?"; return <div className="git-modal__change" key={change.path}><code className={isStaged ? "git-modal__change-state--staged" : ""}>{change.indexStatus}{change.worktreeStatus}</code><span title={change.path}>{change.path}</span><button onClick={() => act(isStaged ? "GitUnstagePaths" : "GitStagePaths", { paths: [change.path] })}>{isStaged ? "Unstage" : "Stage"}</button></div>; })}</div>
    <div className="git-modal__commit"><label htmlFor="git-commit-message">Commit message <span>{staged.length} staged</span></label><textarea id="git-commit-message" value={message} onChange={e => setMessage(e.target.value)} placeholder="Describe this change" rows={2} /></div>
    <div className="modal__actions"><button onClick={onClose}>Close</button><button disabled={!message.trim() || staged.length === 0} onClick={() => act("GitCommit", { message })}>Commit staged</button></div>
  </ResizableModal></div>;
}
