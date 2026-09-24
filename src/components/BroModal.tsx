import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { IconHelpCircle } from "@tabler/icons-react";

import { useInputHistory, type InputHistory } from "../hooks/useInputHistory";
import { useAppDispatch, useAppState, type PendingBro } from "../store";
import { stepNumberInputOnWheel } from "../utils/numberInputWheel";
import type { WsClient } from "../ws/client";
import type { BroParamItem } from "../ws/types";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { ResizableModal } from "./ResizableModal";
import { ModalLayer } from "./ModalStack";
import { isNativeActivationTarget } from "./modalKeyboard";

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

function isFedupBro(item: PendingBro["bros"][number] | undefined) {
  return item?.id === "builtin:fedup";
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
  const [activeFormBroId, setActiveFormBroId] = useState<string | null>(null);
  const [outputs, setOutputs] = useState<Record<string, string>>({});
  const [params, setParams] = useState<Record<string, string>>({});
  const [focusFieldsTick, setFocusFieldsTick] = useState(0);
  const [fedupUseDefaultIgnores, setFedupUseDefaultIgnores] = useState(true);
  const [fedupIncludePatterns, setFedupIncludePatterns] = useState("");
  const [fedupExcludePatterns, setFedupExcludePatterns] = useState("");

  const bros = bro.bros;
  const broParams = selectedBroParams(bros[selectedIndex]);
  const updateFedupIncludePatterns = useCallback((value: string) => {
    setFedupIncludePatterns(value);
    if (value.trim()) {
      setFedupExcludePatterns("");
    }
  }, []);
  const updateFedupExcludePatterns = useCallback((value: string) => {
    setFedupExcludePatterns(value);
    if (value.trim()) {
      setFedupIncludePatterns("");
    }
  }, []);
  const fedupIncludeHistory = useInputHistory("fedup.includePatterns", updateFedupIncludePatterns);
  const fedupExcludeHistory = useInputHistory("fedup.excludePatterns", updateFedupExcludePatterns);

  useEffect(() => {
    if (bros.length === 0) {
      setSelectedIndex(0);
      setActiveFormBroId(null);
      return;
    }
    setSelectedIndex((current) => {
      const nextIndex = Math.min(current, bros.length - 1);
      if (activeFormBroId && !bros.some((bro) => bro.id === activeFormBroId)) {
        setActiveFormBroId(null);
      }
      return nextIndex;
    });
  }, [activeFormBroId, bros]);

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
    if (focusFieldsTick === 0) return;
    const selectedBro = bros[selectedIndex];
    if (!selectedBro || (!selectedBro.output && broParams.length === 0 && !isFedupBro(selectedBro))) return;
    firstFieldRef.current?.focus();
    if (firstFieldRef.current instanceof HTMLInputElement) {
      firstFieldRef.current.select();
    }
  }, [broParams.length, bros, focusFieldsTick, selectedIndex]);

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
  const activeFormBro = useMemo(
    () => bros.find((item) => item.id === activeFormBroId),
    [activeFormBroId, bros],
  );
  const selectedBroIsFedup = isFedupBro(selectedBro);
  const activeFormBroIsFedup = isFedupBro(activeFormBro);
  const selectedBroHasLaunchInputs = !!selectedBro && (
    !!selectedBro.output || selectedBroParams(selectedBro).length > 0 || selectedBroIsFedup
  );
  const selectedBroIsActivated = !!selectedBro && activeFormBro?.id === selectedBro.id;

  const selectedInputValues = useMemo(() => {
    if (!activeFormBro) return {};
    return Object.fromEntries(
      selectedBroParams(activeFormBro).map((input) => [
        input.id,
        params[`${activeFormBro.id}:${input.id}`] ?? input.defaultValue ?? "",
      ]),
    );
  }, [activeFormBro, params]);

  const selectedOutputValue = useMemo(() => {
    if (!activeFormBro?.output) return "";
    return outputs[activeFormBro.id] ?? activeFormBro.output.defaultValue ?? "";
  }, [activeFormBro, outputs]);

  const missingRequiredParam = useMemo(
    () => selectedBroParams(activeFormBro).find((input) => input.required && !selectedInputValues[input.id]?.trim()),
    [activeFormBro, selectedInputValues],
  );

  const outputError = useMemo(() => {
    if (!activeFormBro?.output) return undefined;
    return selectedOutputValue.trim() ? undefined : `${activeFormBro.output.label} is required.`;
  }, [activeFormBro, selectedOutputValue]);

  const paramError = useMemo(() => {
    if (!activeFormBro) return undefined;
    if (activeFormBroIsFedup && fedupIncludePatterns.trim() && fedupExcludePatterns.trim()) {
      return "Fedup include and exclude patterns are mutually exclusive.";
    }
    if (missingRequiredParam) return `${missingRequiredParam.label} is required.`;
    for (const input of selectedBroParams(activeFormBro)) {
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
  }, [activeFormBro, activeFormBroIsFedup, fedupExcludePatterns, fedupIncludePatterns, missingRequiredParam, selectedInputValues]);

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
    if (!selectedBroHasLaunchInputs) {
      launchBro(selectedBro.id);
      return;
    }
    if (!selectedBroIsActivated) {
      activateBroForm(selectedBro, { focusFields: true });
      return;
    }
    if (outputError || paramError) return;
    if (selectedBroIsFedup) {
      if (fedupIncludePatterns.trim()) fedupIncludeHistory.addToHistory(fedupIncludePatterns);
      if (fedupExcludePatterns.trim()) fedupExcludeHistory.addToHistory(fedupExcludePatterns);
      launchBro(selectedBro.id, undefined, {
        useDefaultIgnores: fedupUseDefaultIgnores ? "true" : "false",
        includePatterns: fedupIncludePatterns,
        excludePatterns: fedupExcludePatterns,
      });
      return;
    }
    launchBro(selectedBro.id, selectedBro.output ? selectedOutputValue : undefined, selectedInputValues);
  };

  const activateBroForm = (bro: PendingBro["bros"][number] | undefined, options?: { focusFields?: boolean }) => {
    if (!bro || (!bro.output && selectedBroParams(bro).length === 0 && !isFedupBro(bro))) {
      setActiveFormBroId(null);
      return;
    }
    setActiveFormBroId(bro.id);
    if (options?.focusFields) {
      setFocusFieldsTick((current) => current + 1);
    }
  };

  const selectBro = (index: number) => {
    const bro = bros[index];
    setSelectedIndex(index);
    setActiveFormBroId((current) => (bro && current === bro.id ? current : null));
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.defaultPrevented) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (isNativeActivationTarget(e.target)) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (bros.length > 0) {
        const nextIndex = (selectedIndex + 1) % bros.length;
        selectBro(nextIndex);
      }
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (bros.length > 0) {
        const nextIndex = (selectedIndex - 1 + bros.length) % bros.length;
        selectBro(nextIndex);
      }
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (!selectedBro) return;
      if (!selectedBro.output && selectedBroParams(selectedBro).length === 0 && !selectedBroIsFedup) {
        launchBro(selectedBro.id);
        return;
      }
      if (activeFormBro?.id !== selectedBro.id) {
        activateBroForm(selectedBro, { focusFields: true });
        return;
      }
      launchSelected();
    }
  };

  return (
    <ModalLayer modalId="bro" initialFocusRef={listRef} onClick={close} onKeyDown={onKeyDown}>
      <ResizableModal
        ref={dialogRef}
        modalId="bro"
        client={client}
        className="command-palette open-with"
        ariaModal
        ariaLabel="Pass To Bro"
        testId="bro-dialog"
        tabIndex={-1}
        defaultSize={{ width: 560, height: 500 }}
        minSize={{ width: 420, height: 320 }}
      >
        <div className="modal__header bro-modal__header">
          <span>Pass To Bro...</span>
          <button
            className="bro-modal__help"
            type="button"
            aria-label="About bros"
            title="About bros"
            data-testid="bro-help-button"
            onClick={() => dispatch({ type: "OPEN_SETTINGS", tab: "bros" })}
          >
            <IconHelpCircle size={18} aria-hidden />
          </button>
        </div>
        <div className="modal__body open-with__body">
          <div className="open-with__target">{formatTarget(bro.inputs)}</div>
          <div
            className="command-palette__list open-with__list"
            ref={listRef}
            role="listbox"
            aria-label="Bros"
            aria-activedescendant={bros[selectedIndex] ? `bro-option-${selectedIndex}` : undefined}
            tabIndex={0}
          >
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
                  id={`bro-option-${index}`}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  tabIndex={-1}
                  className={`command-palette__item${selected ? " command-palette__item--selected" : ""}`}
                  data-testid="bro-option"
                  data-bro-id={item.id}
                  data-selected={selected ? "true" : "false"}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    selectBro(index);
                    if (!item.output && selectedBroParams(item).length === 0 && !isFedupBro(item)) {
                      launchBro(item.id);
                      return;
                    }
                    activateBroForm(item, { focusFields: true });
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
          {bro.status === "ready" && selectedBroIsActivated && activeFormBro && (
            activeFormBro.output || selectedBroParams(activeFormBro).length > 0 || activeFormBroIsFedup
          ) && (
            <div className="modal__body" style={{ padding: "12px 0 0" }}>
              {activeFormBroIsFedup && (
                <div className="bro-modal__fedup-options">
                  <label className="bro-modal__fedup-checkbox">
                    <input
                      type="checkbox"
                      data-testid="fedup-default-ignores-input"
                      checked={fedupUseDefaultIgnores}
                      onChange={(e) => setFedupUseDefaultIgnores(e.target.checked)}
                    />
                    Use default ignore patterns
                  </label>
                  <FedupPatternField
                    label="Include only"
                    value={fedupIncludePatterns}
                    placeholder="*.mp4, *.mkv"
                    disabled={!!fedupExcludePatterns.trim()}
                    inputRef={firstFieldRef}
                    testId="fedup-include-patterns-input"
                    history={fedupIncludeHistory}
                    onChange={updateFedupIncludePatterns}
                    onSubmit={launchSelected}
                    onCancel={close}
                  />
                  <FedupPatternField
                    label="Exclude"
                    value={fedupExcludePatterns}
                    placeholder=".git, node_modules, *.tmp"
                    disabled={!!fedupIncludePatterns.trim()}
                    testId="fedup-exclude-patterns-input"
                    history={fedupExcludeHistory}
                    onChange={updateFedupExcludePatterns}
                    onSubmit={launchSelected}
                    onCancel={close}
                  />
                  <div className="bro-modal__fedup-hint">
                    Wildcard names are comma-separated. Exclude patterns skip matching files and folders; include patterns match files only.
                  </div>
                </div>
              )}
              {activeFormBro.output && (
                <div className="modal__rename-row">
                  <label className="modal__rename-label">{activeFormBro.output.label}:</label>
                  <input
                    ref={firstFieldRef}
                    className="modal__rename-input"
                    value={selectedOutputValue}
                    onChange={(e) => {
                      const nextValue = e.target.value;
                      setOutputs((current) => ({
                        ...current,
                        [activeFormBro.id]: nextValue,
                      }));
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
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
              {selectedBroParams(activeFormBro).map((input, index) => {
                const value = selectedInputValues[input.id] ?? "";
                const isFirstField = !activeFormBro.output && index === 0;
                return (
                  <div className="modal__rename-row" key={`${activeFormBro.id}:${input.id}`}>
                    <label className="modal__rename-label">{input.label}:</label>
                    <BroParamField
                      input={input}
                      value={value}
                      inputRef={isFirstField ? firstFieldRef : undefined}
                      onChange={(nextValue) => {
                        setParams((current) => ({
                          ...current,
                          [`${activeFormBro.id}:${input.id}`]: nextValue,
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
            data-testid="bro-launch-button"
            disabled={
              bro.status !== "ready"
              || !selectedBro
              || (selectedBroHasLaunchInputs && !selectedBroIsActivated)
              || !!outputError
              || !!paramError
            }
            onClick={launchSelected}
          >
            Launch
          </button>
          <button className="modal__btn" onClick={close}>
            Cancel
          </button>
        </div>
      </ResizableModal>
    </ModalLayer>
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

interface FedupPatternFieldProps {
  label: string;
  value: string;
  placeholder: string;
  disabled?: boolean;
  inputRef?: React.RefObject<HTMLInputElement | null>;
  testId?: string;
  history: InputHistory;
  onChange: (value: string) => void;
  onSubmit: () => void;
  onCancel: () => void;
}

function FedupPatternField({
  label,
  value,
  placeholder,
  disabled,
  inputRef,
  testId,
  history,
  onChange,
  onSubmit,
  onCancel,
}: FedupPatternFieldProps) {
  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    history.onKeyDown(e);
    if (e.defaultPrevented) return;
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
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

  return (
    <div className="modal__rename-row">
      <label className="modal__rename-label">{label}:</label>
      <input
        ref={(node) => {
          (history.inputRef as React.MutableRefObject<HTMLInputElement | null>).current = node;
          if (inputRef) {
            (inputRef as React.MutableRefObject<HTMLInputElement | null>).current = node;
          }
        }}
        className="search-modal__mask-input"
        data-testid={testId}
        type="text"
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
      />
      {history.isOpen && (
        <InputHistoryDropdown
          anchorEl={history.inputRef.current}
          items={history.items}
          activeIndex={history.activeIndex}
          onSelect={history.selectItem}
          onClose={history.close}
        />
      )}
    </div>
  );
}

function BroParamField({ input, value, inputRef, onChange, onSubmit, onCancel }: BroParamFieldProps) {
  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
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
      onWheel={stepNumberInputOnWheel}
      spellCheck={false}
      autoComplete="off"
      autoCorrect="off"
      autoCapitalize="off"
    />
  );
}
