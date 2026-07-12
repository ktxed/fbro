import { useMemo, useState } from "react";
import type { GitRepoStatus } from "../ws/types";
import type { WsClient } from "../ws/client";

export function GitModal({ panelId, git, client, onClose }: { panelId: string; git: GitRepoStatus; client: WsClient; onClose: () => void }) {
  const [message, setMessage] = useState("");
  const staged = useMemo(() => git.changes.filter(c => c.indexStatus !== " " && c.indexStatus !== "?"), [git]);
  const act = (name: string, args: Record<string, unknown> = {}) => client.sendIntent(name, undefined, { panelId, ...args });
  return <div className="modal-overlay" role="dialog" aria-modal="true"><div className="modal git-modal">
    <div className="modal__header"><h2>Git · {git.branch}</h2><button onClick={onClose}>×</button></div>
    <div className="git-modal__summary">{git.root} · {git.stagedCount} staged · {git.modifiedCount} modified · {git.untrackedCount} untracked</div>
    {!git.clean && <div className="modal__warning">Branch switch, pull, and push require a clean worktree. Commit or stash changes, or continue in VS Code/another Git tool.</div>}
    <div className="git-modal__actions"><select value={git.branch} disabled={!git.clean} onChange={e => act("GitSwitchBranch", { branch: e.target.value })}>{git.branches.map(branch => <option key={branch} value={branch}>{branch}</option>)}</select><button disabled={!git.clean} onClick={() => act("GitPull")}>Pull</button><button disabled={!git.clean} onClick={() => act("GitPush")}>Push</button><button onClick={() => act("GitRefresh")}>Refresh</button></div>
    <div className="git-modal__changes">{git.changes.map(change => { const isStaged = change.indexStatus !== " " && change.indexStatus !== "?"; return <div className="git-modal__change" key={change.path}><code>{change.indexStatus}{change.worktreeStatus}</code><span>{change.path}</span><button onClick={() => act(isStaged ? "GitUnstagePaths" : "GitStagePaths", { paths: [change.path] })}>{isStaged ? "Unstage" : "Stage"}</button></div>; })}</div>
    <textarea value={message} onChange={e => setMessage(e.target.value)} placeholder="Commit message" rows={3} />
    <div className="modal__actions"><button onClick={onClose}>Close</button><button disabled={!message.trim() || staged.length === 0} onClick={() => act("GitCommit", { message })}>Commit staged</button></div>
  </div></div>;
}
