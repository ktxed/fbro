import { useEffect, useRef, useState } from "react";
import { useAppDispatch, useAppState } from "../store";
import type { PendingSearch } from "../store";
import type { Source } from "../ws/types";
import type { WsClient } from "../ws/client";
import { IconX, IconFolder, IconFile } from "@tabler/icons-react";
import { useInputHistory } from "../hooks/useInputHistory";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { DateTimeInput } from "./DateTimeInput";

function MnemonicLabel({ text, mnemonic }: { text: string; mnemonic: string }) {
  const idx = text.toLowerCase().indexOf(mnemonic.toLowerCase());
  if (idx < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, idx)}
      <span className="modal__mnemonic">{text[idx]}</span>
      {text.slice(idx + 1)}
    </>
  );
}

type DepthOption = "unlimited" | "current" | number;
type MaskMode = "substring" | "wildcard" | "regex";
type SizeOp = "none" | "interval" | "gt" | "lt" | "eq";
type TimeOp = "none" | "interval" | "before" | "after" | "relative";
type SizeUnit = "B" | "KiB" | "MiB" | "GiB";
type RelativeUnit = "Minutes" | "Hours" | "Days";

const UNIT_CHAR: Record<RelativeUnit, string> = { Minutes: "m", Hours: "h", Days: "d" };
const CHAR_UNIT: Record<string, RelativeUnit> = { m: "Minutes", h: "Hours", d: "Days" };
const SIZE_UNITS: SizeUnit[] = ["B", "KiB", "MiB", "GiB"];

const pad = (n: number) => String(n).padStart(2, "0");

// Default for the datetime fields: today at 00:00, in datetime-local shape.
function todayMidnight(): string {
  const d = new Date();
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T00:00`;
}

// Epoch-seconds string -> datetime-local shape (local time).
function epochToLocal(epoch: string): string {
  const d = new Date(Number(epoch) * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

// datetime-local shape -> epoch-seconds string.
function localToEpoch(dt: string): string {
  return String(Math.floor(new Date(dt).getTime() / 1000));
}

interface SearchModalProps {
  search: PendingSearch;
  sources: Record<string, Source>;
  client: WsClient;
}

export function SearchModal({ search, sources, client }: SearchModalProps) {
  const dispatch = useAppDispatch();
  const state = useAppState();

  const p = search.searchParams;

  // Section 1 — Search Locations
  const [paths, setPaths] = useState<string[]>(
    p?.paths.length ? p.paths : [search.sourcePanelPath]
  );
  const [pathInput, setPathInput] = useState("");
  const [selectedSourceId, setSelectedSourceId] = useState("");
  const [depth, setDepth] = useState<DepthOption>(() => {
    if (!p) return "unlimited";
    if (p.depth === "unlimited") return "unlimited";
    if (p.depth === "current") return "current";
    const n = Number.parseInt(p.depth, 10);
    return Number.isNaN(n) ? "unlimited" : n;
  });

  // Section 2 — File Name
  const [fileMask, setFileMask] = useState(p?.fileMask ?? "");
  const [maskMode, setMaskMode] = useState<MaskMode>(
    (p?.maskMode as MaskMode | undefined) ?? "wildcard"
  );

  // Section 3 — Content
  const [contentQuery, setContentQuery] = useState(p?.contentQuery ?? "");

  // Section 4 — Extra: File Size
  const [sizeOp, setSizeOp] = useState<SizeOp>(
    (p?.sizeOp as SizeOp | undefined) ?? "none"
  );
  const [sizeValue, setSizeValue] = useState<string>(
    p?.sizeValue != null ? String(p.sizeValue) : ""
  );
  const [sizeMax, setSizeMax] = useState<string>(
    p?.sizeMax != null ? String(p.sizeMax) : ""
  );
  const [sizeUnit, setSizeUnit] = useState<SizeUnit>(
    (p?.sizeUnit as SizeUnit | undefined) ?? "KiB"
  );

  const sizeEnabled = sizeOp !== "none";
  const toggleSizeFilter = (on: boolean) => {
    if (on) {
      setSizeOp("gt");
    } else {
      setSizeOp("none");
      setSizeValue("");
      setSizeMax("");
    }
  };

  // Section 5 — Extra: File Datetime
  const [timeOp, setTimeOp] = useState<TimeOp>(
    (p?.timeOp as TimeOp | undefined) ?? "none"
  );
  // Threshold for Older/Newer than (before/after).
  const [timeValueDT, setTimeValueDT] = useState<string>(() => {
    const v = p?.timeValue;
    const op = (p?.timeOp as TimeOp | undefined) ?? "none";
    if (v && (op === "before" || op === "after") && /^\d+$/.test(v)) return epochToLocal(v);
    return todayMidnight();
  });
  // Start of the Between range (independent of the threshold above).
  const [timeFromDT, setTimeFromDT] = useState<string>(() => {
    const v = p?.timeValue;
    const op = (p?.timeOp as TimeOp | undefined) ?? "none";
    if (v && op === "interval" && /^\d+$/.test(v)) return epochToLocal(v);
    return todayMidnight();
  });
  // End of the Between range.
  const [timeToDT, setTimeToDT] = useState<string>(() => {
    const v = p?.timeTo;
    if (!v || !/^\d+$/.test(v)) return todayMidnight();
    return epochToLocal(v);
  });
  const [timeRelativeNum, setTimeRelativeNum] = useState<string>(() => {
    const v = p?.timeValue;
    if (!v) return "6";
    const op = (p?.timeOp as TimeOp | undefined) ?? "none";
    if (op === "relative") {
      const match = v.match(/^(\d+)[mhd]$/);
      return match ? match[1] : "6";
    }
    return "6";
  });
  const [timeRelativeUnit, setTimeRelativeUnit] = useState<RelativeUnit>(() => {
    const v = p?.timeValue;
    if (!v) return "Hours";
    const op = (p?.timeOp as TimeOp | undefined) ?? "none";
    if (op === "relative") {
      const match = v.match(/^\d+([mhd])$/);
      if (match) return CHAR_UNIT[match[1]] ?? "Hours";
    }
    return "Hours";
  });
  const [timeDateField, setTimeDateField] = useState<"created" | "modified">("modified");

  const timeEnabled = timeOp !== "none";
  const toggleTimeFilter = (on: boolean) => {
    if (on) {
      setTimeOp("relative");
    } else {
      setTimeOp("none");
      setTimeValueDT(todayMidnight());
      setTimeFromDT(todayMidnight());
      setTimeToDT(todayMidnight());
      setTimeRelativeNum("6");
      setTimeRelativeUnit("Hours");
    }
  };

  const RELATIVE_PRESETS = ["5m", "1h", "6h", "24h", "7d", "30d"];

  const applyPreset = (preset: string) => {
    const match = preset.match(/^(\d+)([mhd])$/);
    if (!match) return;
    setTimeRelativeNum(match[1]);
    setTimeRelativeUnit(CHAR_UNIT[match[2]] ?? "Hours");
    setTimeOp("relative");
  };

  const isPresetActive = (preset: string) => {
    if (timeOp !== "relative") return false;
    const match = preset.match(/^(\d+)([mhd])$/);
    if (!match) return false;
    return timeRelativeNum === match[1] && timeRelativeUnit === (CHAR_UNIT[match[2]] ?? "Hours");
  };

  const cycleSizeUnit = () => {
    const idx = SIZE_UNITS.indexOf(sizeUnit);
    setSizeUnit(SIZE_UNITS[(idx + 1) % SIZE_UNITS.length]);
  };

  const maskHistory = useInputHistory("search.fileMask", setFileMask);
  const contentHistory = useInputHistory("search.content", setContentQuery);
  const pathHistory = useInputHistory("search.path", setPathInput);

  const modalRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (search.activeTab === "input") {
      (maskHistory.inputRef as React.RefObject<HTMLInputElement>).current?.focus();
    } else {
      modalRef.current?.focus();
    }
  }, [search.activeTab]);

  const mountedSources = Object.values(sources).filter((s) => s.mounted && !s.shadow);

  const close = () => dispatch({ type: "CLOSE_SEARCH" });

  const addPath = () => {
    const trimmed = pathInput.trim();
    if (!trimmed || paths.includes(trimmed)) return;
    pathHistory.addToHistory(pathInput);
    setPaths((prev) => [...prev, trimmed]);
    setPathInput("");
    setSelectedSourceId("");
  };

  const removePath = (path: string) => {
    setPaths((prev) => prev.filter((p) => p !== path));
  };

  const handleSourceSelect = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const sourceId = e.target.value;
    setSelectedSourceId(sourceId);
    if (!sourceId) return;
    const source = sources[sourceId];
    if (source) setPathInput(source.rootPath);
  };

  const handlePathInputKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    pathHistory.onKeyDown(e);
    if (e.defaultPrevented) return;
    if (e.key === "Enter") {
      e.preventDefault();
      addPath();
    }
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };

  const startSearch = (background: boolean) => {
    if (paths.length === 0) return;
    const requestId = globalThis.crypto?.randomUUID?.() ?? `search-${Date.now()}`;

    maskHistory.addToHistory(fileMask);
    contentHistory.addToHistory(contentQuery);

    dispatch({ type: "RESET_SEARCH_FOR_NEW_JOB", requestId });

    client.sendIntent("StartSearch", undefined, {
      requestId,
      paths,
      depth: typeof depth === "number" ? String(depth) : depth,
      fileMask: fileMask || null,
      maskMode,
      contentQuery: contentQuery.trim() ? contentQuery : null,
      background,
      sizeOp: sizeOp !== "none" ? sizeOp : undefined,
      sizeValue: sizeOp !== "none" && sizeValue ? Number(sizeValue) : undefined,
      sizeMax: sizeOp === "interval" && sizeMax ? Number(sizeMax) : undefined,
      sizeUnit: sizeOp !== "none" ? sizeUnit : undefined,
      timeOp: timeOp !== "none" ? timeOp : undefined,
      timeValue: timeOp !== "none"
        ? (timeOp === "relative"
            ? (timeRelativeNum ? `${timeRelativeNum}${UNIT_CHAR[timeRelativeUnit]}` : null)
            : timeOp === "interval"
              ? (timeFromDT ? localToEpoch(timeFromDT) : null)
              : (timeValueDT ? localToEpoch(timeValueDT) : null))
        : undefined,
      timeTo: timeOp === "interval" && timeToDT
        ? localToEpoch(timeToDT)
        : undefined,
      timeDateField: timeOp !== "none" ? timeDateField : undefined,
    });

    if (background) {
      close();
    } else {
      dispatch({ type: "SET_SEARCH_TAB", tab: "results" });
    }
  };

  const handleCancel = () => {
    if (search.jobId) {
      client.sendIntent("CancelJob", undefined, { jobId: search.jobId });
    }
  };

  const handleToResults = () => {
    if (!search.jobId) return;
    client.sendIntent("MountSearchResults", undefined, { jobId: search.jobId, panelId: state.focus });
    close();
  };

  const handleResultDoubleClick = (fullPath: string, kind: string) => {
    client.sendIntent("NavigateToSearchResult", undefined, {
      panelId: state.focus,
      fullPath,
      kind,
    });
    close();
  };

  const handleOverlayKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (e.key === "Enter") {
      const target = e.target as HTMLElement;
      const isTextarea = target.tagName === "TEXTAREA";
      const isPathInput = target === pathHistory.inputRef.current;
      const isFileMaskInput = target === maskHistory.inputRef.current;
      if (!isTextarea && !isPathInput && !isFileMaskInput) {
        e.preventDefault();
        if (hasResults && search.jobId) {
          handleToResults();
        } else if (search.status === "idle") {
          startSearch(false);
        }
      }
    }
    // Tab shortcuts: s = Search Input, r = Results
    if (e.key === "s" || e.key === "S") {
      const target = e.target as HTMLElement;
      if (target.tagName !== "INPUT" && target.tagName !== "TEXTAREA" && target.tagName !== "SELECT") {
        e.preventDefault();
        dispatch({ type: "SET_SEARCH_TAB", tab: "input" });
      }
    }
    if (e.key === "r" || e.key === "R") {
      const target = e.target as HTMLElement;
      if (target.tagName !== "INPUT" && target.tagName !== "TEXTAREA" && target.tagName !== "SELECT") {
        e.preventDefault();
        dispatch({ type: "SET_SEARCH_TAB", tab: "results" });
      }
    }
    if (e.key === "e" || e.key === "E") {
      const target = e.target as HTMLElement;
      if (target.tagName !== "INPUT" && target.tagName !== "TEXTAREA" && target.tagName !== "SELECT") {
        e.preventDefault();
        dispatch({ type: "SET_SEARCH_TAB", tab: "extra" });
      }
    }
  };

  const depthOptions: { value: string; label: string }[] = [
    { value: "unlimited", label: "Unlimited" },
    { value: "current", label: "Current folder only" },
    ...Array.from({ length: 10 }, (_, i) => ({
      value: String(i + 1),
      label: `Depth ${i + 1}`,
    })),
  ];

  const depthValue =
    depth === "unlimited" ? "unlimited" : depth === "current" ? "current" : String(depth);

  const handleDepthChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const v = e.target.value;
    if (v === "unlimited" || v === "current") {
      setDepth(v);
    } else {
      setDepth(Number.parseInt(v, 10));
    }
  };

  const resultCount = search.results.length;
  const isRunning = search.status === "running";
  const isDone = search.status === "completed" || search.status === "cancelled" || search.status === "failed";
  const hasResults = resultCount > 0;

  return (
      <div
      className="modal-overlay"
      tabIndex={-1}
      onKeyDown={handleOverlayKeyDown}
    >
      <div
        ref={modalRef}
        className="modal modal--wide"
        role="dialog"
        aria-label="Search"
        data-testid="search-dialog"
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal__header">Search</div>

        {/* Tab bar */}
        <div className="search-modal__tabs">
          <button
            type="button"
            className={`search-modal__tab${search.activeTab === "input" ? " search-modal__tab--active" : ""}`}
            onClick={() => dispatch({ type: "SET_SEARCH_TAB", tab: "input" })}
          >
            <MnemonicLabel text="Search Input" mnemonic="s" />
          </button>
          <button
            type="button"
            className={`search-modal__tab${search.activeTab === "results" ? " search-modal__tab--active" : ""}`}
            onClick={() => dispatch({ type: "SET_SEARCH_TAB", tab: "results" })}
          >
            <MnemonicLabel text={`Results${resultCount > 0 ? ` (${resultCount})` : ""}`} mnemonic="r" />
            {isRunning && <span className="search-modal__tab-spinner" />}
          </button>
          <button
            type="button"
            className={`search-modal__tab${search.activeTab === "extra" ? " search-modal__tab--active" : ""}`}
            onClick={() => dispatch({ type: "SET_SEARCH_TAB", tab: "extra" })}
          >
            <MnemonicLabel text="Extra" mnemonic="e" />
          </button>
        </div>

        <div className="modal__body">

          {/* ── Input Tab ─────────────────────────────────────────── */}
          {search.activeTab === "input" && (
            <>
              {/* Section 1: Search Locations */}
              <div className="search-modal__section">
                <div className="search-modal__section-title">Search in</div>

                <div className="search-modal__path-input-row">
                  <input
                    ref={pathHistory.inputRef as React.RefObject<HTMLInputElement>}
                    type="text"
                    className="search-modal__path-input"
                    aria-label="Search path"
                    data-testid="search-path-input"
                    value={pathInput}
                    onChange={(e) => setPathInput(e.target.value)}
                    onKeyDown={handlePathInputKeyDown}
                    placeholder="Add a path…"
                    spellCheck={false}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="off"
                  />
                  {pathHistory.isOpen && (
                    <InputHistoryDropdown
                      anchorEl={pathHistory.inputRef.current}
                      items={pathHistory.items}
                      activeIndex={pathHistory.activeIndex}
                      onSelect={pathHistory.selectItem}
                      onClose={pathHistory.close}
                    />
                  )}
                  {mountedSources.length > 0 && (
                    <select
                      className="search-modal__source-select"
                      value={selectedSourceId}
                      onChange={handleSourceSelect}
                    >
                      <option value="">Volume…</option>
                      {mountedSources.map((source) => (
                        <option key={source.id} value={source.id}>
                          {source.name}
                        </option>
                      ))}
                    </select>
                  )}
                  <button
                    type="button"
                    className="modal__btn search-modal__add-btn"
                    data-command-id="search.path.add"
                    onClick={addPath}
                    disabled={!pathInput.trim() || paths.includes(pathInput.trim())}
                  >
                    Add
                  </button>
                </div>

                {paths.length > 0 && (
                  <ul className="search-modal__path-list">
                    {paths.map((path) => (
                      <li key={path} className="search-modal__path-item">
                        <span
                          className="search-modal__path-text"
                          title={path}
                          onClick={() => setPathInput(path)}
                        >
                          {path}
                        </span>
                        <button
                          type="button"
                          className="search-modal__path-remove"
                          onClick={() => removePath(path)}
                          aria-label={`Remove ${path}`}
                        >
                          <IconX size={10} />
                        </button>
                      </li>
                    ))}
                  </ul>
                )}

                <div className="search-modal__depth-row">
                  <label className="search-modal__depth-label" htmlFor="search-depth">
                    Depth:
                  </label>
                  <select
                    id="search-depth"
                    className="search-modal__depth-select"
                    aria-label="Depth"
                    value={depthValue}
                    onChange={handleDepthChange}
                  >
                    {depthOptions.map((opt) => (
                      <option key={opt.value} value={opt.value}>
                        {opt.label}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              {/* Section 2: File Name */}
              <div className="search-modal__section">
                <div className="search-modal__section-title">File name</div>

                <div className="search-modal__mask-row">
                  <input
                    ref={maskHistory.inputRef as React.RefObject<HTMLInputElement>}
                    type="text"
                    className="search-modal__mask-input"
                    aria-label="File name"
                    data-testid="search-file-mask-input"
                    value={fileMask}
                    onChange={(e) => setFileMask(e.target.value)}
                    onKeyDown={(e) => {
                      maskHistory.onKeyDown(e);
                      if (e.defaultPrevented) return;
                      if (e.key === "Enter" && search.status === "idle") {
                        e.preventDefault();
                        startSearch(false);
                      }
                    }}
                    placeholder="*.*"
                    spellCheck={false}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="off"
                  />
                  {maskHistory.isOpen && (
                    <InputHistoryDropdown
                      anchorEl={maskHistory.inputRef.current}
                      items={maskHistory.items}
                      activeIndex={maskHistory.activeIndex}
                      onSelect={maskHistory.selectItem}
                      onClose={maskHistory.close}
                    />
                  )}
                </div>

                <div className="search-modal__mode-row">
                  {(["substring", "wildcard", "regex"] as MaskMode[]).map((mode) => (
                    <label key={mode} className="search-modal__mode-option">
                      <input
                        type="radio"
                        name="search-mask-mode"
                        value={mode}
                        checked={maskMode === mode}
                        onChange={() => setMaskMode(mode)}
                      />
                      {mode === "substring" ? "Substring" : mode === "wildcard" ? "Wildcard (*)" : "Regex"}
                    </label>
                  ))}
                </div>
              </div>

              {/* Section 3: Content */}
              <div className="search-modal__section search-modal__section--last">
                <div className="search-modal__section-title">Search file contents</div>

                <div className="search-modal__content-body">
                  <textarea
                    ref={contentHistory.inputRef as React.RefObject<HTMLTextAreaElement>}
                    className="search-modal__content-input"
                    aria-label="Text to find"
                    data-testid="search-content-input"
                    value={contentQuery}
                    onChange={(e) => setContentQuery(e.target.value)}
                    onKeyDown={contentHistory.onKeyDown}
                    placeholder="Text to find…"
                    rows={3}
                    spellCheck={false}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="off"
                  />
                  {contentHistory.isOpen && (
                    <InputHistoryDropdown
                      anchorEl={contentHistory.inputRef.current}
                      items={contentHistory.items}
                      activeIndex={contentHistory.activeIndex}
                      onSelect={contentHistory.selectItem}
                      onClose={contentHistory.close}
                    />
                  )}
                </div>
              </div>
            </>
          )}

          {/* ── Extra Tab ──────────────────────────────────────────── */}
          {search.activeTab === "extra" && (
            <>
              {/* Section: File Size */}
              <div className={`search-modal__section${sizeEnabled ? " search-modal__section--active" : ""}`}>
                <div className="search-modal__extra-row">
                  <label className="search-modal__extra-toggle">
                    <input
                      type="checkbox"
                      checked={sizeEnabled}
                      onChange={(e) => toggleSizeFilter(e.target.checked)}
                    />
                    <span className="search-modal__radio-label-text search-modal__section-title">File size</span>
                  </label>
                  {sizeEnabled && (
                    <>
                      <select
                        className="search-modal__extra-select"
                        value={sizeOp}
                        onChange={(e) => setSizeOp(e.target.value as SizeOp)}
                      >
                        <option value="gt">Bigger than</option>
                        <option value="lt">Smaller than</option>
                        <option value="eq">Equal to</option>
                        <option value="interval">In range</option>
                      </select>
                      <input
                        type="number"
                        className="search-modal__extra-number search-modal__size-num"
                        aria-label="File size value"
                        value={sizeValue}
                        onChange={(e) => setSizeValue(e.target.value)}
                        min="0"
                        placeholder="Value"
                      />
                      {sizeOp === "interval" && (
                        <input
                          type="number"
                          className="search-modal__extra-number search-modal__size-num"
                          aria-label="File size max"
                          value={sizeMax}
                          onChange={(e) => setSizeMax(e.target.value)}
                          min="0"
                          placeholder="Max"
                        />
                      )}
                      <button
                        type="button"
                        className="search-modal__unit-badge"
                        onClick={cycleSizeUnit}
                        aria-label="Size unit, click to change"
                        title="Click to change unit"
                      >
                        {sizeUnit}
                      </button>
                    </>
                  )}
                </div>
              </div>

              {/* Section: File Datetime */}
              <div className={`search-modal__section search-modal__section--last${timeEnabled ? " search-modal__section--active" : ""}`}>
                <div className="search-modal__section-header">
                  <label className="search-modal__extra-toggle">
                    <input
                      type="checkbox"
                      checked={timeEnabled}
                      onChange={(e) => toggleTimeFilter(e.target.checked)}
                    />
                    <span className="search-modal__section-title">File datetime</span>
                  </label>
                  {timeEnabled && (
                    <div className="search-modal__seg-group">
                      <button
                        type="button"
                        className={`search-modal__seg-btn${timeDateField === "created" ? " search-modal__seg-btn--active" : ""}`}
                        onClick={() => setTimeDateField("created")}
                      >
                        Created
                      </button>
                      <button
                        type="button"
                        className={`search-modal__seg-btn${timeDateField === "modified" ? " search-modal__seg-btn--active" : ""}`}
                        onClick={() => setTimeDateField("modified")}
                      >
                        Modified
                      </button>
                    </div>
                  )}
                </div>

                {timeEnabled && (
                  <div className="search-modal__radio-options">
                    {/* Radio 1: In the last */}
                    <div className="search-modal__radio-option">
                      <label className="search-modal__radio-label">
                        <input
                          type="radio"
                          name="timeOp"
                          checked={timeOp === "relative"}
                          onChange={() => setTimeOp("relative")}
                        />
                        <span>In the last...</span>
                        <input
                          type="number"
                          className="search-modal__extra-number search-modal__relative-num"
                          aria-label="Relative time amount"
                          value={timeRelativeNum}
                          onChange={(e) => setTimeRelativeNum(e.target.value)}
                          min="1"
                          onClick={() => setTimeOp("relative")}
                        />
                        <select
                          className="search-modal__extra-select"
                          value={timeRelativeUnit}
                          onChange={(e) => {
                            setTimeRelativeUnit(e.target.value as RelativeUnit);
                            setTimeOp("relative");
                          }}
                        >
                          <option value="Minutes">Minutes</option>
                          <option value="Hours">Hours</option>
                          <option value="Days">Days</option>
                        </select>
                      </label>
                      <div className="search-modal__extra-presets">
                        {RELATIVE_PRESETS.map((preset) => (
                          <button
                            key={preset}
                            type="button"
                            className={`search-modal__extra-preset${isPresetActive(preset) ? " search-modal__extra-preset--active" : ""}`}
                            onClick={() => applyPreset(preset)}
                          >
                            {preset}
                          </button>
                        ))}
                      </div>
                    </div>

                    {/* Radio 2: Older than */}
                    <label className="search-modal__radio-label search-modal__radio-label--spaced">
                      <input
                        type="radio"
                        name="timeOp"
                        checked={timeOp === "before" || timeOp === "after"}
                        onChange={() => setTimeOp(timeOp === "after" ? "after" : "before")}
                      />
                      <select
                        className="search-modal__extra-select search-modal__radio-label-text"
                        value={timeOp === "after" ? "after" : "before"}
                        onChange={(e) => setTimeOp(e.target.value as TimeOp)}
                      >
                        <option value="before">Older than</option>
                        <option value="after">Newer than</option>
                      </select>
                      <DateTimeInput
                        ariaLabel="Date threshold"
                        value={timeValueDT}
                        onChange={setTimeValueDT}
                        onInteract={() => {
                          if (timeOp !== "before" && timeOp !== "after") setTimeOp("before");
                        }}
                      />
                    </label>

                    {/* Radio 3: Between */}
                    <label className="search-modal__radio-label search-modal__radio-label--spaced">
                      <input
                        type="radio"
                        name="timeOp"
                        checked={timeOp === "interval"}
                        onChange={() => setTimeOp("interval")}
                      />
                      <span className="search-modal__radio-label-text">Between</span>
                      <DateTimeInput
                        ariaLabel="Between from date"
                        value={timeFromDT}
                        onChange={setTimeFromDT}
                        onInteract={() => setTimeOp("interval")}
                      />
                      <DateTimeInput
                        ariaLabel="Between to date"
                        value={timeToDT}
                        onChange={setTimeToDT}
                        onInteract={() => setTimeOp("interval")}
                      />
                    </label>
                  </div>
                )}
              </div>
            </>
          )}

          {/* ── Results Tab ───────────────────────────────────────── */}
          {search.activeTab === "results" && (
            <div className="search-modal__results-pane">
              {isRunning && (
                <div className="search-modal__progress-info">
                  <span className="search-modal__progress-counts">
                    {search.matched} result{search.matched !== 1 ? "s" : ""} found
                    {search.scanned > 0 && `, ${search.scanned} items scanned`}
                  </span>
                  {search.currentPath && (
                    <span className="search-modal__progress-path" title={search.currentPath}>
                      Scanning: {search.currentPath}
                    </span>
                  )}
                </div>
              )}
              {isDone && hasResults && (
                <div className="search-modal__progress-info search-modal__progress-info--done">
                  {search.matched} result{search.matched !== 1 ? "s" : ""} found
                  {search.status === "cancelled" ? " (search cancelled)" : ""}
                </div>
              )}

              {hasResults ? (
                <ul className="search-modal__results">
                  {search.results.slice(0, 500).map((result) => (
                    <li
                      key={result.fullPath}
                      className="search-modal__result-row"
                      onDoubleClick={() => handleResultDoubleClick(result.fullPath, result.kind)}
                      title={result.fullPath}
                    >
                      <span className={`search-modal__result-icon ${result.kind === "directory" ? "cc-dir" : ""}`}>
                        {result.kind === "directory" ? <IconFolder size={14} /> : <IconFile size={14} />}
                      </span>
                      <span className="search-modal__result-main">
                        <span className="search-modal__result-name">{result.name}</span>
                        <span className="search-modal__result-path">{result.parentPath}</span>
                      </span>
                      <button
                        type="button"
                        className="modal__job-param-copy"
                        title="Copy full path"
                        onClick={(e) => {
                          e.stopPropagation();
                          navigator.clipboard.writeText(result.fullPath);
                        }}
                      >⧉</button>
                    </li>
                  ))}
                </ul>
              ) : isDone ? (
                <div className="search-modal__empty">No results found.</div>
              ) : isRunning ? (
                <div className="search-modal__empty">Searching…</div>
              ) : (
                <div className="search-modal__empty">Press Search to begin.</div>
              )}
            </div>
          )}

        </div>

        {/* Results cap warning */}
        {search.activeTab === "results" && resultCount > 500 && (
          <div className="search-modal__cap-warning">
            Showing first 500 of {resultCount} results. Use "To Results" to browse all.
          </div>
        )}

        {/* Action buttons */}
        <div className="modal__actions">
          {(search.status === "idle" || isDone) && !(hasResults && search.activeTab === "results") && (
            <>
              <button
                className="modal__btn modal__btn--primary"
                data-command-id="search.start"
                onClick={() => startSearch(false)}
                disabled={paths.length === 0}
              >
                Search
              </button>
              <button
                className="modal__btn"
                data-command-id="search.background"
                onClick={() => startSearch(true)}
                disabled={paths.length === 0}
              >
                <MnemonicLabel text="Background" mnemonic="b" />
              </button>
            </>
          )}
          {isRunning && (
            <button className="modal__btn" data-command-id="search.cancel" onClick={handleCancel}>
              Cancel Search
            </button>
          )}
          {hasResults && (
            <button
              className="modal__btn modal__btn--primary"
              data-command-id="search.to-results"
              onClick={handleToResults}
              disabled={!search.jobId}
            >
              To Results
            </button>
          )}
          <button className="modal__btn" data-command-id="search.close" onClick={close}>
            <MnemonicLabel text="Close" mnemonic="c" />
          </button>
        </div>
      </div>
    </div>
  );
}
