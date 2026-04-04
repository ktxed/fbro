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

function selectedBroInputs(item: PendingBro["bros"][number] | undefined) {
  return item?.inputs ?? [];
}

export function BroModal({ client, bro }: BroModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const dialogRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [launchInputs, setLaunchInputs] = useState<Record<string, string>>({});

  const bros = bro.bros;
  const broInputs = selectedBroInputs(bros[selectedIndex]);

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
    const selectedBro = bros[selectedIndex];
    const inputs = selectedBroInputs(selectedBro);
    if (!selectedBro || inputs.length === 0) return;
    setLaunchInputs((current) => {
      const next = { ...current };
      for (const input of inputs) {
        const key = `${selectedBro.id}:${input.id}`;
        if (next[key] === undefined) {
          next[key] = input.defaultValue ?? "";
        }
      }
      return next;
    });
  }, [bros, selectedIndex]);

  useEffect(() => {
    const selectedBro = bros[selectedIndex];
    if (!selectedBro || broInputs.length === 0) return;
    firstInputRef.current?.focus();
    firstInputRef.current?.select();
  }, [broInputs.length, bros, selectedIndex]);

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

  const selectedInputValues = useMemo(() => {
    if (!selectedBro) return {};
    return Object.fromEntries(
      broInputs.map((input) => [
        input.id,
        launchInputs[`${selectedBro.id}:${input.id}`] ?? input.defaultValue ?? "",
      ]),
    );
  }, [broInputs, launchInputs, selectedBro]);

  const missingRequiredInput = useMemo(
    () => broInputs.find((input) => input.required && !selectedInputValues[input.id]?.trim()),
    [broInputs, selectedInputValues],
  );

  const launchBro = (broId: string, inputs: Record<string, string> = {}) => {
    close();
    client.sendIntent("LaunchBro", undefined, {
      requestId: bro.requestId,
      broId,
      inputs,
    });
  };

  const launchSelected = () => {
    if (!selectedBro) return;
    if (missingRequiredInput) return;
    launchBro(selectedBro.id, selectedInputValues);
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
                    onClick={() => {
                      setSelectedIndex(index);
                      if (selectedBroInputs(item).length === 0) {
                        launchBro(item.id);
                      }
                    }}
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
          {bro.status === "ready" && selectedBro && broInputs.length > 0 && (
            <div className="modal__body" style={{ padding: "12px 0 0" }}>
              {broInputs.map((input, index) => {
                const value = selectedInputValues[input.id] ?? "";
                return (
                  <div className="modal__rename-row" key={`${selectedBro.id}:${input.id}`}>
                    <label className="modal__rename-label">{input.label}:</label>
                    <input
                      ref={index === 0 ? firstInputRef : undefined}
                      className="modal__rename-input"
                      value={value}
                      onChange={(e) => {
                        const nextValue = e.target.value;
                        setLaunchInputs((current) => ({
                          ...current,
                          [`${selectedBro.id}:${input.id}`]: nextValue,
                        }));
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "ArrowUp" || e.key === "ArrowDown" || e.key === "Tab") {
                          e.stopPropagation();
                        }
                        if (e.key === "Enter") {
                          e.preventDefault();
                          e.stopPropagation();
                          launchSelected();
                        }
                        if (e.key === "Escape") {
                          e.preventDefault();
                          e.stopPropagation();
                          close();
                        }
                      }}
                      spellCheck={false}
                      autoComplete="off"
                      autoCorrect="off"
                      autoCapitalize="off"
                    />
                  </div>
                );
              })}
              {missingRequiredInput && (
                <p className="modal__rename-error">{missingRequiredInput.label} is required.</p>
              )}
            </div>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            disabled={bro.status !== "ready" || !selectedBro || !!missingRequiredInput}
            onClick={launchSelected}
          >
            Launch
          </button>
          <button className="modal__btn" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
