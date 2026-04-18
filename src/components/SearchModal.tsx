import { useEffect, useRef, useState } from "react";
import { useAppDispatch, useAppState } from "../store";
import type { PendingSearch } from "../store";
import type { Source } from "../ws/types";
import type { WsClient } from "../ws/client";

type DepthOption = "unlimited" | "current" | number;
type MaskMode = "substring" | "wildcard" | "regex";

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
  const [contentEnabled, setContentEnabled] = useState(!!p?.contentQuery);
  const [contentQuery, setContentQuery] = useState(p?.contentQuery ?? "");

  const pathInputRef = useRef<HTMLInputElement>(null);
  const contentTextareaRef = useRef<HTMLTextAreaElement>(null);
  const modalRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (search.activeTab === "input") {
      pathInputRef.current?.focus();
    } else {
      modalRef.current?.focus();
    }
  }, [search.activeTab]);

  const mountedSources = Object.values(sources).filter((s) => s.mounted && !s.shadow);

  const close = () => dispatch({ type: "CLOSE_SEARCH" });

  const addPath = () => {
    const trimmed = pathInput.trim();
    if (!trimmed || paths.includes(trimmed)) return;
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

    // Reset accumulated results so the Results tab starts fresh for the new job.
    dispatch({ type: "RESET_SEARCH_FOR_NEW_JOB", requestId });

    client.sendIntent("StartSearch", undefined, {
      requestId,
      paths,
      depth: typeof depth === "number" ? String(depth) : depth,
      fileMask: fileMask || null,
      maskMode,
      contentQuery: contentEnabled ? contentQuery : null,
      background,
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
    client.sendIntent("MountSearchResults", undefined, { jobId: search.jobId });
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
    if (e.key === "Tab") {
      e.preventDefault();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (e.key === "Enter") {
      const target = e.target as HTMLElement;
      const isTextarea = target.tagName === "TEXTAREA";
      const isPathInput = target === pathInputRef.current;
      if (!isTextarea && !isPathInput && search.activeTab === "input" && search.status === "idle") {
        e.preventDefault();
        startSearch(false);
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
            Search Input
          </button>
          <button
            type="button"
            className={`search-modal__tab${search.activeTab === "results" ? " search-modal__tab--active" : ""}`}
            onClick={() => dispatch({ type: "SET_SEARCH_TAB", tab: "results" })}
          >
            Results{resultCount > 0 ? ` (${resultCount})` : ""}
            {isRunning && <span className="search-modal__tab-spinner" />}
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
                    ref={pathInputRef}
                    type="text"
                    className="search-modal__path-input"
                    value={pathInput}
                    onChange={(e) => setPathInput(e.target.value)}
                    onKeyDown={handlePathInputKeyDown}
                    placeholder="Add a path…"
                    spellCheck={false}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="off"
                  />
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
                        <span className="search-modal__path-text" title={path}>{path}</span>
                        <button
                          type="button"
                          className="search-modal__path-remove"
                          onClick={() => removePath(path)}
                          aria-label={`Remove ${path}`}
                        >
                          ✕
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
                    type="text"
                    className="search-modal__mask-input"
                    value={fileMask}
                    onChange={(e) => setFileMask(e.target.value)}
                    placeholder="*.*"
                    spellCheck={false}
                    autoComplete="off"
                    autoCorrect="off"
                    autoCapitalize="off"
                  />
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
                <div
                  className="search-modal__section-header"
                  onClick={() => {
                    setContentEnabled((v) => {
                      if (!v) setTimeout(() => contentTextareaRef.current?.focus(), 0);
                      return !v;
                    });
                  }}
                >
                  <input
                    type="checkbox"
                    checked={contentEnabled}
                    onChange={() => {}}
                    onClick={(e) => e.stopPropagation()}
                    onFocus={(e) => e.stopPropagation()}
                  />
                  <span className="search-modal__section-title search-modal__section-title--inline">
                    Search file contents
                  </span>
                </div>

                {contentEnabled && (
                  <div className="search-modal__content-body">
                    <textarea
                      ref={contentTextareaRef}
                      className="search-modal__content-input"
                      value={contentQuery}
                      onChange={(e) => setContentQuery(e.target.value)}
                      placeholder="Text to find…"
                      rows={3}
                      spellCheck={false}
                      autoComplete="off"
                      autoCorrect="off"
                      autoCapitalize="off"
                    />
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
                  {search.results.map((result) => (
                    <li
                      key={result.fullPath}
                      className="search-modal__result-row"
                      onDoubleClick={() => handleResultDoubleClick(result.fullPath, result.kind)}
                      title={result.fullPath}
                    >
                      <span className={`search-modal__result-icon ${result.kind === "directory" ? "cc-dir" : ""}`}>
                        {result.kind === "directory" ? "📁" : "📄"}
                      </span>
                      <span className="search-modal__result-main">
                        <span className="search-modal__result-name">{result.name}</span>
                        <span className="search-modal__result-path">{result.parentPath}</span>
                      </span>
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

        {/* Action buttons */}
        <div className="modal__actions">
          {(search.status === "idle" || isDone) && !(hasResults && search.activeTab === "results") && (
            <>
              <button
                className="modal__btn modal__btn--primary"
                onClick={() => startSearch(false)}
                disabled={paths.length === 0}
              >
                Search
              </button>
              <button
                className="modal__btn"
                onClick={() => startSearch(true)}
                disabled={paths.length === 0}
              >
                Background
              </button>
            </>
          )}
          {isRunning && (
            <button className="modal__btn" onClick={handleCancel}>
              Cancel Search
            </button>
          )}
          {hasResults && (
            <button
              className="modal__btn"
              onClick={handleToResults}
              disabled={!search.jobId}
            >
              To Results
            </button>
          )}
          <button className="modal__btn" onClick={close}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
