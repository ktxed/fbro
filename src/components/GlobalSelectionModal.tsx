import { useEffect, useRef, useState } from "react";
import { useAppDispatch, useAppState, type PendingGlobalSelectionAdd } from "../store";
import type { WsClient } from "../ws/client";
import { ModalLayer } from "./ModalStack";
import { isNativeActivationTarget } from "./modalKeyboard";

interface GlobalSelectionModalProps {
  add: PendingGlobalSelectionAdd;
  client: WsClient;
}

const CREATE_NEW = "__create__";

function formatCreatedAt(createdAt: string | undefined): string {
  if (!createdAt) return "";
  const secs = parseInt(createdAt, 10);
  if (isNaN(secs)) return "";
  const d = new Date(secs * 1000);
  const p = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function GlobalSelectionModal({ add, client }: GlobalSelectionModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const [target, setTarget] = useState(state.globalSelections[0]?.id ?? CREATE_NEW);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const selectRef = useRef<HTMLSelectElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const creating = target === CREATE_NEW;
  const summaryName = add.targetPath
    ? add.targetName
    : `${add.targetCount} selected ${add.targetCount === 1 ? "entry" : "entries"}`;

  useEffect(() => {
    if (creating) inputRef.current?.focus();
    else selectRef.current?.focus();
  }, [creating]);

  const close = () => dispatch({ type: "CLOSE_GLOBAL_SELECTION_ADD" });

  const submit = () => {
    const trimmed = name.trim();
    if (creating) {
      if (trimmed.includes("/") || trimmed.includes("\\")) {
        setError("Selection name cannot contain path separators.");
        return;
      }
      if (trimmed && state.globalSelections.some((selection) => selection.name === trimmed)) {
        setError("A global selection with this name already exists.");
        return;
      }
    }

    client.sendIntent("AddToGlobalSelection", add.panelId, {
      panelId: add.panelId,
      ...(add.targetPath ? { targetPath: add.targetPath } : {}),
      ...(creating ? { name: trimmed } : { selectionId: target }),
    });
    close();
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    if (e.defaultPrevented) return;
    if (e.key === "Enter" && !isNativeActivationTarget(e.target)) {
      e.preventDefault();
      submit();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };

  const handleNameKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "ArrowUp" || name.trim()) return;
    const previousSelection = state.globalSelections[state.globalSelections.length - 1];
    if (!previousSelection) return;
    e.preventDefault();
    setTarget(previousSelection.id);
    setError(null);
  };

  return (
    <ModalLayer modalId="global-selection" onClick={close} onKeyDown={handleKeyDown}>
      <div className="modal" role="dialog" aria-label="To global selection" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">To global selection</div>
        <div className="modal__body">
          <p className="modal__summary">
            Add <strong>{summaryName}</strong>
          </p>
          <div className="modal__rename-row">
            <label className="modal__rename-label">Selection:</label>
            <select
              ref={selectRef}
              className="modal__rename-input"
              value={target}
              onChange={(e) => {
                setTarget(e.target.value);
                setError(null);
              }}
            >
              {state.globalSelections.map((selection) => (
                <option key={selection.id} value={selection.id}>
                  {selection.name} ({selection.itemCount}) - {formatCreatedAt(selection.createdAt)}
                </option>
              ))}
              <option value={CREATE_NEW}>Create new selection</option>
            </select>
          </div>
          {creating && (
            <div className="modal__rename-row">
              <label className="modal__rename-label">Name:</label>
              <input
                ref={inputRef}
                className="modal__rename-input"
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  setError(null);
                }}
                onKeyDown={handleNameKeyDown}
                placeholder="Selection N"
                spellCheck={false}
              />
            </div>
          )}
          {error && <p className="modal__rename-error">{error}</p>}
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" onClick={submit}>
            Add
          </button>
          <button className="modal__btn" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </ModalLayer>
  );
}
