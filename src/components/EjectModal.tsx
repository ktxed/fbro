import { useRef } from "react";
import { useAppDispatch, useAppState, type EjectAction } from "../store";
import type { WsClient } from "../ws/client";
import { ModalDialog } from "./ModalDialog";
import { ModalLayer } from "./ModalStack";
import { isNativeActivationTarget } from "./modalKeyboard";

export function EjectModal({ client }: { client: WsClient }) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const pending = state.pendingEject!;
  const selectRef = useRef<HTMLSelectElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const sources = Object.values(state.sources).filter((source) => source.mounted && source.eject);
  const source = pending.sourceId === null
    ? sources[0]
    : sources.find((candidate) => candidate.id === pending.sourceId);
  const action: EjectAction = pending.action === "unmount-volume" || source?.eject?.canEjectDrive === false
    ? "unmount-volume"
    : "eject-drive";
  const busy = pending.requestId !== null;

  const close = () => dispatch({ type: "CLOSE_EJECT" });
  const select = (sourceId: string, nextAction?: EjectAction) => {
    const next = sources.find((candidate) => candidate.id === sourceId);
    if (!next) return;
    dispatch({
      type: "SET_EJECT_SELECTION",
      sourceId,
      action: nextAction ?? (next.eject?.canEjectDrive ? "eject-drive" : "unmount-volume"),
    });
  };
  const submit = () => {
    if (!source?.eject || busy) return;
    const requestId = globalThis.crypto?.randomUUID?.() ?? `eject-${Date.now()}`;
    dispatch({ type: "SUBMIT_EJECT", requestId, sourceId: source.id, action });
    client.sendIntent("EjectSource", undefined, {
      sourceId: source.id,
      action,
      requestId,
      expectedMountedVolumeCount: source.eject.mountedVolumeCount,
    });
  };
  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.defaultPrevented) return;
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    } else if (event.key === "Enter" && !isNativeActivationTarget(event.target)) {
      event.preventDefault();
      submit();
    }
  };

  return (
    <ModalLayer modalId="eject-source" initialFocusRef={source ? selectRef : cancelRef} onClick={close} onKeyDown={onKeyDown}>
      <ModalDialog ariaLabel="Eject removable source" className="eject-modal" testId="eject-modal" onClick={(event) => event.stopPropagation()}>
        <div className="modal__header">Eject removable source</div>
        <div className="modal__body eject-modal__body">
          {source ? (
            <>
              <label htmlFor="eject-source-list">Removable source</label>
              <select
                id="eject-source-list"
                ref={selectRef}
                className="eject-modal__list"
                size={Math.min(6, Math.max(2, sources.length))}
                value={source.id}
                disabled={busy}
                onChange={(event) => select(event.target.value)}
              >
                {sources.map((candidate) => (
                  <option key={candidate.id} value={candidate.id}>{candidate.name} — {candidate.rootPath}</option>
                ))}
              </select>
              <fieldset className="eject-modal__actions">
                <legend>Action</legend>
                <label>
                  <input type="radio" name="eject-action" checked={action === "unmount-volume"} disabled={busy} onChange={() => select(source.id, "unmount-volume")} />
                  Unmount selected volume
                </label>
                {source.eject?.canEjectDrive && (
                  <label>
                    <input type="radio" name="eject-action" checked={action === "eject-drive"} disabled={busy} onChange={() => select(source.id, "eject-drive")} />
                    Eject entire drive
                  </label>
                )}
              </fieldset>
              {action === "eject-drive" && (
                <p className="eject-modal__warning" role="alert">
                  Ejecting {source.eject?.driveName || source.name} will unmount {source.eject?.mountedVolumeCount} {source.eject?.mountedVolumeCount === 1 ? "volume" : "volumes"} on this drive. Open tabs on those volumes will return Home.
                </p>
              )}
            </>
          ) : <p>{sources.length ? "The selected source is no longer mounted. Reopen the picker to choose another source." : "No mounted removable sources are available."}</p>}
          {pending.error && <p className="modal__rename-error" role="alert">{pending.error}</p>}
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" data-testid="eject-submit" disabled={!source || busy} onClick={submit}>
            {busy ? (action === "eject-drive" ? "Ejecting…" : "Unmounting…") : (action === "eject-drive" ? "Eject drive" : "Unmount volume")}
          </button>
          <button ref={cancelRef} className="modal__btn" onClick={close}>Cancel</button>
        </div>
      </ModalDialog>
    </ModalLayer>
  );
}
