import { useEffect, useMemo, useRef, useState } from "react";
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
  const canCommit = message.trim().length > 0 && staged.length > 0;
  const act = (name: string, args: Record<string, unknown> = {}) => client.sendIntent(name, undefined, { panelId, ...args });
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
      const accelerators: Record<string, () => void> = {
        b: () => branchRef.current?.focus(),
        l: () => { if (git.clean) act("GitPull"); },
        p: () => { if (git.clean) act("GitPush"); },
        r: () => act("GitRefresh"),
        c: () => { if (canCommit) commitRef.current?.click(); },
      };
      const action = accelerators[event.key.toLowerCase()];
      if (action) {
        event.preventDefault();
        event.stopImmediatePropagation();
        action();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [canCommit, git.clean, onClose]);
  return <div className="modal-overlay" role="dialog" aria-modal="true" onKeyDown={(event) => event.stopPropagation()} onKeyUp={(event) => event.stopPropagation()}><ResizableModal ref={modalRef} modalId="git" client={client} className="git-modal" ariaLabel="Git repository" ariaModal defaultSize={{ width: 760, height: 700 }} minSize={{ width: 640, height: 620 }}>
    <div className="git-modal__header"><div><span className="git-modal__eyebrow">REPOSITORY</span><strong>{git.branch}</strong></div><button ref={closeRef} className="git-modal__close" title="Close (Esc)" onClick={onClose}>×</button></div>
    <div className="git-modal__summary" title={git.root}><span className="git-modal__path">{git.root}</span><span className="git-modal__stat">{git.stagedCount} staged</span><span className="git-modal__stat">{git.modifiedCount} modified</span><span className="git-modal__stat">{git.untrackedCount} untracked</span></div>
    {!git.clean && <div className="git-modal__warning">Working tree has changes. Branch switch, pull, and push are disabled; use another Git tool to stash or resolve them.</div>}
    <div className="git-modal__toolbar"><select ref={branchRef} value={git.branch} disabled={!git.clean} onChange={e => act("GitSwitchBranch", { branch: e.target.value })}>{git.branches.map(branch => <option key={branch} value={branch}>{branch}</option>)}</select><kbd>B</kbd><span className="git-modal__toolbar-spacer" /><button disabled={!git.clean} onClick={() => act("GitPull")}>Pull <kbd>L</kbd></button><button disabled={!git.clean} onClick={() => act("GitPush")}>Push <kbd>P</kbd></button><button onClick={() => act("GitRefresh")}>Refresh <kbd>R</kbd></button></div>
    <div className="git-modal__section-title">Changes <span>{git.changes.length}</span></div>
    <div className="git-modal__changes">{git.changes.map(change => { const isStaged = change.indexStatus !== " " && change.indexStatus !== "?"; return <div className="git-modal__change" key={change.path}><code className={isStaged ? "git-modal__change-state--staged" : ""}>{change.indexStatus}{change.worktreeStatus}</code><span title={change.path}>{change.path}</span><button onClick={() => act(isStaged ? "GitUnstagePaths" : "GitStagePaths", { paths: [change.path] })}>{isStaged ? "Unstage" : "Stage"}</button></div>; })}</div>
    <div className="git-modal__commit"><label htmlFor="git-commit-message">Commit message <span>{staged.length} staged</span></label><textarea id="git-commit-message" value={message} onChange={e => setMessage(e.target.value)} placeholder="Describe this change" rows={2} /></div>
    <div className="modal__actions"><button onClick={onClose}>Close</button><button ref={commitRef} disabled={!canCommit} onClick={() => act("GitCommit", { message })}>Commit staged <kbd>C</kbd></button></div>
  </ResizableModal></div>;
}
