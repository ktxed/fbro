import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { RgbaColorPicker } from "react-colorful";
import type { GeneralSettings } from "../ws/types";
import type { WsClient } from "../ws/client";
import { useAppDispatch, useAppState } from "../store";
import { listColorSchemes, resolveColorScheme, type TerminalColors } from "../theme/colorSchemes";
import { formatColorLike, isValidColor, parseColor } from "../theme/colorValue";
import {
  captureRenderedTokens,
  COLOR_TOKENS,
  TERMINAL_TOKENS,
  TOKEN_GROUPS,
  type TokenGroupId,
} from "../theme/tokens";
import { rovingIndexForKey } from "./modalKeyboard";
import { ColorSchemePreview } from "./ColorSchemePreview";

interface Props {
  general: GeneralSettings;
  client: WsClient;
}

interface Draft {
  id: string;
  name: string;
  description: string;
  mode: "dark" | "light";
  /** Token name → colour, plus `terminal.<key>` entries. Always the full set. */
  values: Record<string, string>;
}

interface TokenRow {
  id: string;
  label: string;
  description: string;
  group: TokenGroupId;
}

const TERMINAL_PREFIX = "terminal.";
const SAVE_DELAY_MS = 300;

const ROWS: TokenRow[] = [
  ...COLOR_TOKENS.map((t) => ({ id: t.name, label: t.label, description: t.description, group: t.group })),
  ...TERMINAL_TOKENS.map((t) => ({
    id: `${TERMINAL_PREFIX}${t.key}`,
    label: t.label,
    description: t.description,
    group: "terminal" as const,
  })),
];

function terminalValues(terminal: TerminalColors): Record<string, string> {
  return {
    [`${TERMINAL_PREFIX}background`]: terminal.background,
    [`${TERMINAL_PREFIX}foreground`]: terminal.foreground,
    [`${TERMINAL_PREFIX}cursor`]: terminal.cursor,
    [`${TERMINAL_PREFIX}selectionBackground`]: terminal.selectionBackground,
  };
}

/** Split `values` into the wire shape, dropping entries that would be rejected. */
function toPayload(values: Record<string, string>) {
  const colors: Record<string, string> = {};
  const terminal: Record<string, string> = {};
  for (const [key, value] of Object.entries(values)) {
    if (!isValidColor(value)) continue;
    if (key.startsWith(TERMINAL_PREFIX)) terminal[key.slice(TERMINAL_PREFIX.length)] = value;
    else colors[key] = value;
  }
  return { colors, terminal };
}

type Prompt = null | "duplicate" | "delete";
type SaveStatus = "idle" | "saving" | "saved" | "error";

export function SettingsColorSchemeTab({ general, client }: Props) {
  const { connected, focus } = useAppState();
  const dispatch = useAppDispatch();
  const catalog = general.colorSchemes;
  const schemes = useMemo(() => listColorSchemes(catalog), [catalog]);

  const [selectedId, setSelectedId] = useState(() => resolveColorScheme(general.colorScheme, catalog).id);
  const scheme = resolveColorScheme(selectedId, catalog);
  const exists = schemes.some((s) => s.id === selectedId);
  const readOnly = !scheme.custom;

  const probeRef = useRef<HTMLDivElement>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const draftRef = useRef<Draft | null>(null);
  const originalRef = useRef<Draft | null>(null);
  const dirtyRef = useRef(false);
  const timerRef = useRef<number | undefined>(undefined);

  const [tokenId, setTokenId] = useState<string>(ROWS[0].id);
  const [filter, setFilter] = useState("");
  const [rawText, setRawText] = useState("");
  const [prompt, setPrompt] = useState<Prompt>(null);
  const [duplicateName, setDuplicateName] = useState("");
  const [status, setStatus] = useState<SaveStatus>("idle");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const pendingRequest = useRef<{ id: string; kind: "update" | "create" | "delete" } | null>(null);
  const pendingSelect = useRef<string | null>(null);
  const rowRefs = useRef(new Map<string, HTMLButtonElement>());
  const textRef = useRef<HTMLInputElement>(null);
  const duplicateBtnRef = useRef<HTMLButtonElement>(null);
  const deleteBtnRef = useRef<HTMLButtonElement>(null);
  const duplicateInputRef = useRef<HTMLInputElement>(null);

  // ── Saving ──────────────────────────────────────────────────────────────
  const flush = useCallback(() => {
    window.clearTimeout(timerRef.current);
    const d = draftRef.current;
    if (!d || !dirtyRef.current) return;
    dirtyRef.current = false;
    const requestId = crypto.randomUUID();
    pendingRequest.current = { id: requestId, kind: "update" };
    setStatus("saving");
    const { colors, terminal } = toPayload(d.values);
    client.sendIntent("UpdateColorScheme", undefined, {
      requestId,
      id: d.id,
      ...(d.name.trim() ? { name: d.name.trim() } : {}),
      description: d.description,
      mode: d.mode,
      colors,
      terminal,
    });
  }, [client]);

  const commit = useCallback((next: Draft) => {
    draftRef.current = next;
    dirtyRef.current = true;
    setDraft(next);
    setStatus("idle");
    window.clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(flush, SAVE_DELAY_MS);
  }, [flush]);

  // Flush pending edits when leaving the tab (unmount) or the scheme.
  useEffect(() => flush, [flush]);

  // ── Selection / draft initialisation ────────────────────────────────────
  useLayoutEffect(() => {
    flush();
    const probe = probeRef.current;
    if (!probe || !exists) return;
    const current = resolveColorScheme(selectedId, catalog);
    const captured = captureRenderedTokens(probe);
    const values = {
      ...captured,
      ...(current.custom ? (catalog?.items.find((i) => i.id === selectedId)?.colors ?? {}) : {}),
      ...terminalValues(current.terminal),
    };
    const initial: Draft = { id: selectedId, name: current.label, description: current.custom ? (catalog?.items.find((i) => i.id === selectedId)?.description ?? "") : current.description, mode: current.mode, values };
    draftRef.current = initial;
    originalRef.current = initial;
    dirtyRef.current = false;
    setDraft(initial);
    setStatus("idle");
    setError(undefined);
    setPrompt(null);
    // `catalog` is intentionally not a dependency: later patches (our own
    // saves, folder changes) must not clobber an in-progress draft.
  }, [selectedId, exists]); // eslint-disable-line react-hooks/exhaustive-deps

  // The selected scheme vanished (deleted here or in the folder): fall back.
  useEffect(() => {
    if (exists) return;
    if (pendingSelect.current === selectedId) return;
    setSelectedId(resolveColorScheme(general.colorScheme, catalog).id);
  }, [exists, selectedId, general.colorScheme, catalog]);

  // A just-created scheme becomes selectable once its catalog patch arrives.
  useEffect(() => {
    const wanted = pendingSelect.current;
    if (wanted && schemes.some((s) => s.id === wanted)) {
      pendingSelect.current = null;
      setSelectedId(wanted);
    }
  }, [schemes]);

  // ── Replies ─────────────────────────────────────────────────────────────
  useEffect(() => client.subscribeMessages((message) => {
    if (message.type !== "ColorSchemeSaved" && message.type !== "ColorSchemeFailed") return;
    const pending = pendingRequest.current;
    if (!pending || pending.id !== message.requestId) return;
    pendingRequest.current = null;
    setBusy(false);
    if (message.type === "ColorSchemeFailed") {
      setStatus("error");
      setError(message.message);
      return;
    }
    setError(undefined);
    setStatus(pending.kind === "update" ? "saved" : "idle");
    if (pending.kind === "create") {
      setPrompt(null);
      pendingSelect.current = message.id;
      if (schemes.some((s) => s.id === message.id)) {
        pendingSelect.current = null;
        setSelectedId(message.id);
      }
      duplicateBtnRef.current?.focus();
    }
    if (pending.kind === "delete") {
      setPrompt(null);
      deleteBtnRef.current?.focus();
    }
  }), [client, schemes]);

  useEffect(() => {
    if (!connected && pendingRequest.current) {
      pendingRequest.current = null;
      setBusy(false);
      setStatus("error");
      setError("Connection lost. Reconnect to try again.");
    }
  }, [connected]);

  // ── Token editor ────────────────────────────────────────────────────────
  const visibleRows = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return ROWS;
    return ROWS.filter((r) => `${r.label} ${r.description} ${r.id}`.toLowerCase().includes(q));
  }, [filter]);

  const row = ROWS.find((r) => r.id === tokenId) ?? ROWS[0];
  const value = draft?.values[row.id] ?? "";
  useEffect(() => { setRawText(value); }, [row.id, value]);

  const setValue = (id: string, next: string) => {
    if (readOnly || !draft) return;
    commit({ ...draft, values: { ...draft.values, [id]: next } });
  };

  const onText = (text: string) => {
    setRawText(text);
    if (isValidColor(text)) setValue(row.id, text.trim());
  };

  const rgba = parseColor(value) ?? { r: 0, g: 0, b: 0, a: 1 };

  const focusRow = (id: string) => {
    setTokenId(id);
    rowRefs.current.get(id)?.focus();
    rowRefs.current.get(id)?.scrollIntoView({ block: "nearest" });
  };

  const onRowKeyDown = (e: React.KeyboardEvent, index: number) => {
    const next = rovingIndexForKey(e.key, index, visibleRows.length, "vertical");
    if (next !== null) {
      e.preventDefault();
      focusRow(visibleRows[next].id);
      return;
    }
    if (e.key === "Enter" && !readOnly) {
      e.preventDefault();
      textRef.current?.focus();
      textRef.current?.select();
    }
  };

  // Preview click: select the token and show it in the list.
  const selectFromPreview = (id: string) => {
    setFilter("");
    setTokenId(id);
    requestAnimationFrame(() => rowRefs.current.get(id)?.scrollIntoView({ block: "nearest" }));
  };

  // Escape unwinds nested state first, then falls through to close Settings.
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "Escape") return;
    if (prompt) {
      e.preventDefault();
      e.stopPropagation();
      closePrompt();
      return;
    }
    const target = e.target as HTMLElement;
    if (target.closest("[data-color-editor]")) {
      e.preventDefault();
      e.stopPropagation();
      rowRefs.current.get(row.id)?.focus();
    }
  };

  const closePrompt = () => {
    const was = prompt;
    setPrompt(null);
    (was === "delete" ? deleteBtnRef : duplicateBtnRef).current?.focus();
  };

  // ── Actions ─────────────────────────────────────────────────────────────
  const startDuplicate = () => {
    setDuplicateName(`${scheme.label} copy`);
    setPrompt("duplicate");
  };
  useEffect(() => {
    if (prompt === "duplicate") {
      duplicateInputRef.current?.focus();
      duplicateInputRef.current?.select();
    }
  }, [prompt]);

  const submitDuplicate = () => {
    const name = duplicateName.trim();
    if (!name || busy || !connected || !draft) return;
    flush();
    setBusy(true);
    setError(undefined);
    const requestId = crypto.randomUUID();
    pendingRequest.current = { id: requestId, kind: "create" };
    const { colors, terminal } = toPayload(draft.values);
    client.sendIntent("CreateColorScheme", undefined, {
      requestId,
      name,
      description: `Based on ${scheme.label}`,
      mode: draft.mode,
      base: scheme.base,
      colors,
      terminal,
    });
  };

  const submitDelete = () => {
    if (busy || !connected || readOnly) return;
    window.clearTimeout(timerRef.current);
    dirtyRef.current = false;
    setBusy(true);
    setError(undefined);
    const requestId = crypto.randomUUID();
    pendingRequest.current = { id: requestId, kind: "delete" };
    client.sendIntent("DeleteColorScheme", undefined, { requestId, id: selectedId });
  };

  const revert = () => {
    const original = originalRef.current;
    if (!original || readOnly) return;
    commit({ ...original });
    flush();
  };

  const useScheme = () => {
    flush();
    client.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: selectedId });
  };

  const openFolder = () => {
    if (!catalog?.dir || !focus) return;
    client.sendIntent("NavigatePanelTo", undefined, { panelId: focus, path: catalog.dir });
    dispatch({ type: "CLOSE_SETTINGS" });
  };

  const followingSystem = general.followSystemColorScheme && general.systemColorScheme !== null;
  const isManual = general.colorScheme === selectedId;
  const dirty = !!draft && !!originalRef.current && JSON.stringify(draft) !== JSON.stringify(originalRef.current);

  const grouped = TOKEN_GROUPS.map((group) => ({
    group,
    rows: visibleRows.filter((r) => r.group === group.id),
  })).filter((g) => g.rows.length > 0);

  return (
    <div className="settings-tab color-editor" data-testid="color-scheme-editor" onKeyDown={onKeyDown}>
      {/* Probe: renders the selected scheme's tokens so they can be read back. */}
      <div
        ref={probeRef}
        hidden
        data-color-scheme={scheme.id}
        data-color-scheme-base={scheme.base}
        aria-hidden="true"
      />

      <div className="settings-tab__section">
        <h3 className="settings-tab__section-title">Color Scheme</h3>
        <div className="color-editor__bar">
          <label className="settings-tab__option">
            <span className="settings-tab__select-label">Scheme</span>
            <select
              className="settings-tab__select"
              data-testid="color-editor-scheme-select"
              value={exists ? selectedId : ""}
              onChange={(e) => { flush(); setSelectedId(e.target.value); }}
            >
              {schemes.map((s) => (
                <option key={s.id} value={s.id}>{s.custom ? `${s.label} (custom)` : s.label}</option>
              ))}
            </select>
          </label>
          <button
            className="modal__btn"
            type="button"
            data-testid="color-editor-use"
            disabled={isManual || !connected}
            onClick={useScheme}
            title={followingSystem ? "Applies when “Match system appearance” is off" : undefined}
          >
            {isManual ? "In use" : "Use this scheme"}
          </button>
        </div>

        <div className="color-editor__bar">
          <button ref={duplicateBtnRef} className={`modal__btn${readOnly ? " modal__btn--primary" : ""}`} type="button" data-testid="color-editor-duplicate" disabled={!connected || busy || !draft} onClick={startDuplicate}>
            Duplicate…
          </button>
          <button ref={deleteBtnRef} className="modal__btn" type="button" data-testid="color-editor-delete" disabled={readOnly || !connected || busy} onClick={() => setPrompt("delete")}>
            Delete…
          </button>
          <button className="modal__btn" type="button" data-testid="color-editor-revert" disabled={readOnly || !dirty} onClick={revert}>
            Revert
          </button>
          <button className="modal__btn" type="button" data-testid="color-editor-open-folder" disabled={!catalog?.dir || !focus} onClick={openFolder} title={catalog?.dir}>
            Open themes folder
          </button>
          <span className="color-editor__status" role="status" aria-live="polite" data-testid="color-editor-status">
            {status === "saving" ? "Saving…" : status === "saved" ? "Saved" : ""}
          </span>
        </div>

        {prompt === "duplicate" && (
          <form
            className="color-editor__prompt"
            onSubmit={(e) => { e.preventDefault(); submitDuplicate(); }}
          >
            <label className="settings-tab__option">
              <span>Name</span>
              <input
                ref={duplicateInputRef}
                className="settings-tab__text-input"
                data-testid="color-editor-duplicate-name"
                value={duplicateName}
                maxLength={60}
                onChange={(e) => setDuplicateName(e.target.value)}
              />
            </label>
            <button className="modal__btn modal__btn--primary" type="submit" data-testid="color-editor-duplicate-confirm" disabled={busy || !duplicateName.trim()}>
              {busy ? "Creating…" : "Create"}
            </button>
            <button className="modal__btn" type="button" onClick={closePrompt}>Cancel</button>
          </form>
        )}
        {prompt === "delete" && (
          <div className="color-editor__prompt" role="alertdialog" aria-label={`Delete ${scheme.label}`}>
            <span>Delete “{scheme.label}”? This removes its file.</span>
            <button className="modal__btn modal__btn--primary" type="button" data-testid="color-editor-delete-confirm" disabled={busy} onClick={submitDelete} autoFocus>
              {busy ? "Deleting…" : "Delete"}
            </button>
            <button className="modal__btn" type="button" onClick={closePrompt}>Cancel</button>
          </div>
        )}

        {readOnly ? (
          <p className="settings-tab__hint" data-testid="color-editor-readonly-hint">
            {scheme.label} is built in and read-only. Duplicate it to create a scheme you can edit.
          </p>
        ) : draft && (
          <div className="color-editor__meta">
            <label className="settings-tab__option">
              <span className="settings-tab__select-label">Name</span>
              <input className="settings-tab__text-input" data-testid="color-editor-name" value={draft.name} maxLength={60} onChange={(e) => commit({ ...draft, name: e.target.value })} />
            </label>
            <label className="settings-tab__option">
              <span className="settings-tab__select-label">Mode</span>
              <select className="settings-tab__select" data-testid="color-editor-mode" value={draft.mode} onChange={(e) => commit({ ...draft, mode: e.target.value as "dark" | "light" })}>
                <option value="dark">Dark</option>
                <option value="light">Light</option>
              </select>
            </label>
            <label className="settings-tab__option">
              <span className="settings-tab__select-label">About</span>
              <input className="settings-tab__text-input" value={draft.description} maxLength={200} onChange={(e) => commit({ ...draft, description: e.target.value })} />
            </label>
          </div>
        )}
        {error && <p className="settings-tab__hint color-editor__error" role="alert" data-testid="color-editor-error">{error}</p>}
        {catalog && catalog.errors.length > 0 && (
          <div className="settings-tab__hint color-editor__file-errors" data-testid="color-editor-file-errors">
            <strong>Could not load:</strong>
            <ul>
              {catalog.errors.map((e) => <li key={e.file}><code>{e.file}</code>: {e.message}</li>)}
            </ul>
          </div>
        )}
      </div>

      {draft && (
        <div className="color-editor__main">
          <div className="color-editor__tokens">
            <input
              className="settings-tab__text-input color-editor__filter"
              type="search"
              placeholder="Filter colors…"
              aria-label="Filter colors"
              data-testid="color-editor-filter"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
            <div className="color-editor__list" role="listbox" aria-label="Colors" data-testid="color-editor-list">
              {grouped.map(({ group, rows }) => (
                <div key={group.id} role="group" aria-label={group.label}>
                  <div className="color-editor__group" aria-hidden="true">{group.label}</div>
                  {rows.map((r) => {
                    const index = visibleRows.indexOf(r);
                    const selected = r.id === row.id;
                    const v = draft.values[r.id] ?? "";
                    return (
                      <button
                        key={r.id}
                        ref={(node) => { if (node) rowRefs.current.set(r.id, node); else rowRefs.current.delete(r.id); }}
                        type="button"
                        role="option"
                        aria-selected={selected}
                        tabIndex={selected || (!visibleRows.some((x) => x.id === row.id) && index === 0) ? 0 : -1}
                        data-token-row={r.id}
                        className={`color-editor__row${selected ? " color-editor__row--selected" : ""}`}
                        title={r.description}
                        onClick={() => setTokenId(r.id)}
                        onFocus={() => setTokenId(r.id)}
                        onKeyDown={(e) => onRowKeyDown(e, index)}
                      >
                        <span className="color-editor__swatch" style={{ ["--sw" as string]: isValidColor(v) ? v : "transparent" }} aria-hidden="true" />
                        <span className="color-editor__label">{r.label}</span>
                        <code className="color-editor__value">{v}</code>
                      </button>
                    );
                  })}
                </div>
              ))}
              {grouped.length === 0 && <div className="settings-tab__hint">No colors match.</div>}
            </div>

            <div className="color-editor__edit" data-color-editor="" data-testid="color-editor-edit">
              <div className="color-editor__edit-title">{row.label}</div>
              <div className="settings-tab__hint">{row.description}</div>
              <RgbaColorPicker
                className="color-editor__picker"
                color={rgba}
                onChange={(next) => { if (!readOnly) setValue(row.id, formatColorLike(next, value)); }}
                style={readOnly ? { pointerEvents: "none", opacity: 0.6 } : undefined}
              />
              <label className="settings-tab__option">
                <span className="settings-tab__select-label">Value</span>
                <input
                  ref={textRef}
                  className="settings-tab__text-input color-editor__text"
                  data-testid="color-editor-value"
                  value={rawText}
                  readOnly={readOnly}
                  spellCheck={false}
                  aria-invalid={!isValidColor(rawText)}
                  aria-label={`${row.label} value`}
                  onChange={(e) => onText(e.target.value)}
                  onBlur={() => setRawText(value)}
                />
              </label>
            </div>
          </div>

          <div className="color-editor__preview">
            <ColorSchemePreview
              values={draft.values}
              mode={draft.mode}
              base={scheme.base}
              selectedToken={row.id}
              onSelectToken={selectFromPreview}
            />
          </div>
        </div>
      )}
    </div>
  );
}
