import { useEffect, useMemo, useRef, useState } from "react";

import { useAppDispatch, useAppState, type PendingBro } from "../store";
import type { WsClient } from "../ws/client";

interface BroModalProps {
  client: WsClient;
  bro: PendingBro;
}

function formatTarget(inputs: PendingBro["inputs"]) {
  if (inputs.length === 0) return "Resolving inputs...";
  if (inputs.length === 1) return inputs[0].name;
  return `${inputs.length} items (${inputs[0].name} first)`;
}

export function BroModal({ client, bro }: BroModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const dialogRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);

  const bros = bro.bros;

  useEffect(() => {
    dialogRef.current?.focus();
  }, []);

  useEffect(() => {
    if (bros.length === 0) {
      setSelectedIndex(0);
      return;
    }
    setSelectedIndex((current) => Math.min(current, bros.length - 1));
  }, [bros]);

  useEffect(() => {
    const selected = listRef.current?.querySelector<HTMLElement>("[data-selected='true']");
    selected?.scrollIntoView({ block: "nearest" });
  }, [bros, selectedIndex]);

  useEffect(() => {
    const panel = state.panels[bro.panelId];
    if (!panel || panel.path.replace(/[/\\]+$/, "") !== bro.dirPath) {
      dispatch({ type: "CLOSE_BRO" });
    }
  }, [bro.dirPath, bro.panelId, dispatch, state.panels]);

  const close = () => dispatch({ type: "CLOSE_BRO" });

  const selectedBro = useMemo(
    () => bros[selectedIndex],
    [bros, selectedIndex],
  );

  const launchBro = (broId: string) => {
    close();
    client.sendIntent("LaunchBro", undefined, {
      requestId: bro.requestId,
      broId,
    });
  };

  const launchSelected = () => {
    if (!selectedBro) return;
    launchBro(selectedBro.id);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (bros.length > 0) {
        setSelectedIndex((current) => (current + 1) % bros.length);
      }
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (bros.length > 0) {
        setSelectedIndex((current) => (current - 1 + bros.length) % bros.length);
      }
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      launchSelected();
    }
  };

  return (
    <div className="modal-overlay" onClick={close}>
      <div
        ref={dialogRef}
        className="modal command-palette open-with"
        role="dialog"
        aria-modal="true"
        aria-label="Pass To Bro"
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <div className="modal__header">Pass To Bro...</div>
        <div className="modal__body open-with__body">
          <div className="open-with__target">{formatTarget(bro.inputs)}</div>
          <div className="command-palette__list open-with__list" ref={listRef}>
            {bro.status === "loading" && (
              <div className="command-palette__empty">Looking up bros...</div>
            )}
            {bro.status === "error" && (
              <div className="command-palette__empty">{bro.error ?? "Failed to load bros."}</div>
            )}
            {bro.status === "ready" && bros.length === 0 && (
              <div className="command-palette__empty">No bros available.</div>
            )}
            {bro.status === "ready" && bros.map((item, index) => {
              const selected = index === selectedIndex;
              return (
                <button
                  key={item.id}
                  type="button"
                  className={`command-palette__item${selected ? " command-palette__item--selected" : ""}`}
                  data-selected={selected ? "true" : "false"}
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseEnter={() => setSelectedIndex(index)}
                  onClick={() => launchBro(item.id)}
                >
                  <span className="command-palette__item-main">
                    <span className="command-palette__item-title">{item.name} {item.version}</span>
                    <span className="command-palette__item-category">
                      {item.detail ?? item.source}
                    </span>
                  </span>
                  <span className="command-palette__item-binding">{item.protocol}</span>
                </button>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
