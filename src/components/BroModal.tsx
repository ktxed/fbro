import { useEffect, useMemo, useRef, useState } from "react";

import { useAppDispatch, useAppState, type PendingBro } from "../store";
import type { WsClient } from "../ws/client";
import type { BroParamItem } from "../ws/types";

interface BroModalProps {
  client: WsClient;
  bro: PendingBro;
}

function formatTarget(inputs: PendingBro["inputs"]) {
  if (inputs.length === 0) return "Resolving inputs...";
  if (inputs.length === 1) return inputs[0].name;
  return `${inputs.length} items (${inputs[0].name} first)`;
}

function selectedBroParams(item: PendingBro["bros"][number] | undefined) {
  return item?.params ?? [];
}

function broMetaLine(item: PendingBro["bros"][number]) {
  const detail = item.detail ?? item.source;
  if (!item.pre?.reason) return detail;
  return `${item.pre.reason} | ${detail}`;
}

export function BroModal({ client, bro }: BroModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const dialogRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const firstFieldRef = useRef<HTMLInputElement>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [outputs, setOutputs] = useState<Record<string, string>>({});
  const [params, setParams] = useState<Record<string, string>>({});

  const bros = bro.bros;
  const broParams = selectedBroParams(bros[selectedIndex]);

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
    const paramsForBro = selectedBroParams(selectedBro);
    if (!selectedBro) return;
    setOutputs((current) => {
      if (selectedBro.output === undefined || current[selectedBro.id] !== undefined) {
        return current;
      }
      return {
        ...current,
        [selectedBro.id]: selectedBro.output.defaultValue ?? "",
      };
    });
    if (paramsForBro.length === 0) return;
    setParams((current) => {
      const next = { ...current };
      for (const input of paramsForBro) {
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
    if (!selectedBro || (!selectedBro.output && broParams.length === 0)) return;
    firstFieldRef.current?.focus();
    if (firstFieldRef.current instanceof HTMLInputElement) {
      firstFieldRef.current.select();
    }
  }, [broParams.length, bros, selectedIndex]);

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
      broParams.map((input) => [
        input.id,
        params[`${selectedBro.id}:${input.id}`] ?? input.defaultValue ?? "",
      ]),
    );
  }, [broParams, params, selectedBro]);

  const selectedOutputValue = useMemo(() => {
    if (!selectedBro?.output) return "";
    return outputs[selectedBro.id] ?? selectedBro.output.defaultValue ?? "";
  }, [outputs, selectedBro]);

  const missingRequiredParam = useMemo(
    () => broParams.find((input) => input.required && !selectedInputValues[input.id]?.trim()),
    [broParams, selectedInputValues],
  );

  const outputError = useMemo(() => {
    if (!selectedBro?.output) return undefined;
    return selectedOutputValue.trim() ? undefined : `${selectedBro.output.label} is required.`;
  }, [selectedBro, selectedOutputValue]);

  const paramError = useMemo(() => {
    if (!selectedBro) return undefined;
    if (missingRequiredParam) return `${missingRequiredParam.label} is required.`;
    for (const input of broParams) {
      if (input.kind !== "int") continue;
      const rawValue = selectedInputValues[input.id]?.trim();
      if (!rawValue) continue;
      if (!/^-?\d+$/.test(rawValue)) return `${input.label} must be a whole number.`;
      const value = Number(rawValue);
      if (input.minValue !== undefined && value < input.minValue) {
        return `${input.label} must be at least ${input.minValue}.`;
      }
      if (input.maxValue !== undefined && value > input.maxValue) {
        return `${input.label} must be at most ${input.maxValue}.`;
      }
    }
    return undefined;
  }, [broParams, missingRequiredParam, selectedBro, selectedInputValues]);

  const launchBro = (broId: string, output?: string, launchParams: Record<string, string> = {}) => {
    close();
    client.sendIntent("LaunchBro", undefined, {
      requestId: bro.requestId,
      broId,
      output,
      params: launchParams,
    });
  };

  const launchSelected = () => {
    if (!selectedBro) return;
    if (outputError || paramError) return;
    launchBro(selectedBro.id, selectedBro.output ? selectedOutputValue : undefined, selectedInputValues);
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
                      if (!item.output && selectedBroParams(item).length === 0) {
                        launchBro(item.id);
                      }
                    }}
                  title={item.detail}
                >
                  <span className="command-palette__item-main">
                    <span className="command-palette__item-title">{item.name} {item.version}</span>
                    <span className="command-palette__item-category">
                      {broMetaLine(item)}
                    </span>
                  </span>
                  <span className="command-palette__item-binding">{item.protocol}</span>
                </button>
              );
            })}
          </div>
          {bro.status === "ready" && selectedBro && (selectedBro.output || broParams.length > 0) && (
            <div className="modal__body" style={{ padding: "12px 0 0" }}>
              {selectedBro.output && (
                <div className="modal__rename-row">
                  <label className="modal__rename-label">{selectedBro.output.label}:</label>
                  <input
                    ref={firstFieldRef}
                    className="modal__rename-input"
                    value={selectedOutputValue}
                    onChange={(e) => {
                      const nextValue = e.target.value;
                      setOutputs((current) => ({
                        ...current,
                        [selectedBro.id]: nextValue,
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
              )}
              {broParams.map((input, index) => {
                const value = selectedInputValues[input.id] ?? "";
                const isFirstField = !selectedBro.output && index === 0;
                return (
                  <div className="modal__rename-row" key={`${selectedBro.id}:${input.id}`}>
                    <label className="modal__rename-label">{input.label}:</label>
                    <BroParamField
                      input={input}
                      value={value}
                      inputRef={isFirstField ? firstFieldRef : undefined}
                      onChange={(nextValue) => {
                        setParams((current) => ({
                          ...current,
                          [`${selectedBro.id}:${input.id}`]: nextValue,
                        }));
                      }}
                      onSubmit={launchSelected}
                      onCancel={close}
                    />
                  </div>
                );
              })}
              {(outputError || paramError) && (
                <p className="modal__rename-error">{outputError ?? paramError}</p>
              )}
            </div>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            disabled={bro.status !== "ready" || !selectedBro || !!outputError || !!paramError}
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

interface BroParamFieldProps {
  input: BroParamItem;
  value: string;
  inputRef?: React.RefObject<HTMLInputElement | null>;
  onChange: (value: string) => void;
  onSubmit: () => void;
  onCancel: () => void;
}

function BroParamField({ input, value, inputRef, onChange, onSubmit, onCancel }: BroParamFieldProps) {
  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowUp" || e.key === "ArrowDown" || e.key === "Tab") {
      e.stopPropagation();
    }
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      onSubmit();
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    }
  };

  if (input.kind === "password") {
    return (
      <input
        ref={inputRef as React.RefObject<HTMLInputElement>}
        className="modal__rename-input"
        type="password"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
      />
    );
  }

  return (
    <input
      ref={inputRef as React.RefObject<HTMLInputElement>}
      className="modal__rename-input"
      type="number"
      inputMode="numeric"
      min={input.minValue}
      max={input.maxValue}
      step={1}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      onKeyDown={onKeyDown}
      spellCheck={false}
      autoComplete="off"
      autoCorrect="off"
      autoCapitalize="off"
    />
  );
}
