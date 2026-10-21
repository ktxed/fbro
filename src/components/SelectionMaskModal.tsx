import { useCallback, useEffect, useRef, useState } from "react";
import { useAppDispatch, useAppState, type PendingSelectionMask } from "../store";
import { useInputHistory } from "../hooks/useInputHistory";
import { selectionMaskForEntry } from "../utils/selectionMask";
import type { WsClient } from "../ws/client";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { ModalLayer, useModalStack } from "./ModalStack";
import { ModalDialog } from "./ModalDialog";
import { isNativeActivationTarget } from "./modalKeyboard";

interface SelectionMaskModalProps {
  selectionMask: PendingSelectionMask;
  client: WsClient;
}

export function SelectionMaskModal({ selectionMask: pending, client }: SelectionMaskModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const { topModalId } = useModalStack();
  const panel = state.panels[pending.panelId];
  const [value, setValue] = useState(pending.initialMask ?? "");
  const [initialized, setInitialized] = useState(pending.initialMask !== null);
  const [submitting, setSubmitting] = useState(false);
  const edited = useRef(false);
  const submitted = useRef(false);
  const selectedInitialText = useRef(false);
  const requestedEpoch = useRef<number | null>(null);
  const history = useInputHistory("selectionMask", (value) => {
    edited.current = true;
    setValue(value);
  });
  const title = pending.mode === "select" ? "Select Files" : "Deselect Files";
  const verb = pending.mode === "select" ? "Select" : "Deselect";
  const validPanel = !!panel && panel.sourceId === pending.sourceId && panel.path === pending.path && panel.tabKind === "listing";
  const ready = validPanel && panel.status === "ready" && initialized;
  const close = () => dispatch({ type: "CLOSE_SELECTION_MASK" });

  useEffect(() => {
    if (!validPanel) dispatch({ type: "CLOSE_SELECTION_MASK" });
  }, [validPanel, dispatch]);

  useEffect(() => {
    if (!validPanel || initialized || panel.status !== "ready") return;
    const entry = panel.entries[panel.cursor - (panel.windowStart ?? 0)];
    if (entry || (panel.totalCount ?? panel.entries.length) === 0) {
      if (!edited.current) setValue(selectionMaskForEntry(entry));
      setInitialized(true);
    } else if (panel.totalCount !== undefined && requestedEpoch.current !== panel.panelEpoch) {
      requestedEpoch.current = panel.panelEpoch;
      client.sendIntent("RequestWindow", pending.panelId, {
        start: Math.max(0, panel.cursor - 100), count: 200,
      });
    }
  }, [validPanel, initialized, panel, pending.panelId, client]);

  const selectInitialText = useCallback(() => {
    if (initialized && !edited.current && !selectedInitialText.current && document.activeElement === history.inputRef.current) {
      history.inputRef.current?.select();
      selectedInitialText.current = true;
    }
  }, [initialized, history.inputRef]);

  // ModalLayer owns focus. Select only our initial text, never a later edit or
  // text in a covered dialog whose input has deliberately lost focus.
  useEffect(selectInitialText, [selectInitialText]);

  const submit = () => {
    if (submitted.current || !ready || !value.trim()) return;
    submitted.current = true;
    setSubmitting(true);
    history.addToHistory(value);
    client.sendIntent(pending.mode === "select" ? "SelectByMask" : "DeselectByMask", pending.panelId, { mask: value });
    close();
  };

  return (
    <ModalLayer modalId="selection-mask" initialFocusRef={history.inputRef} onClick={close} onKeyDown={(event) => {
      if (event.defaultPrevented) return;
      if (event.target === history.inputRef.current || event.key === "Escape") history.onKeyDown(event);
      if (event.defaultPrevented) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        close();
      } else if (event.key === "Enter" && !event.repeat
        && (event.target === history.inputRef.current || !isNativeActivationTarget(event.target))) {
        event.preventDefault();
        event.stopPropagation();
        submit();
      }
    }}>
      <ModalDialog ariaLabel={title} testId="selection-mask-dialog" onClick={(event) => event.stopPropagation()}>
        <div className="modal__header">{title}</div>
        <div className="modal__body">
          <div className="modal__rename-row">
            <label className="modal__rename-label" htmlFor="selection-mask-input">Mask:</label>
            <input
              id="selection-mask-input"
              data-testid="selection-mask-input"
              ref={history.inputRef as React.RefObject<HTMLInputElement>}
              className="modal__rename-input"
              value={value}
              placeholder={initialized ? undefined : "Loading…"}
              onFocus={selectInitialText}
              onChange={(event) => { edited.current = true; setValue(event.target.value); }}
              spellCheck={false}
              autoComplete="off"
              autoCapitalize="off"
              autoCorrect="off"
              aria-describedby="selection-mask-help"
            />
            {history.isOpen && topModalId === "selection-mask" && (
              <InputHistoryDropdown anchorEl={history.inputRef.current} items={history.items}
                activeIndex={history.activeIndex} onSelect={history.selectItem} onClose={history.close} />
            )}
          </div>
          <p id="selection-mask-help">Separate patterns with spaces. Use * for any text and ? for one character. Files only; ignores case.</p>
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" data-command-id="panel.selection.mask.confirm"
            disabled={submitting || !ready || !value.trim()} onClick={submit}>
            {submitting ? `${verb}ing…` : verb}
          </button>
          <button className="modal__btn" data-command-id="panel.selection.mask.cancel" onClick={close}>Cancel</button>
        </div>
      </ModalDialog>
    </ModalLayer>
  );
}
