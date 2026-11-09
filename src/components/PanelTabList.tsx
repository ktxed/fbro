import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { IconAlertTriangle, IconPencil, IconSearch, IconTerminal2, IconX } from "@tabler/icons-react";
import { editorStatus, editorStatusVersion, subscribeEditorStatus } from "../editor/sessions";
import { useAppDispatch } from "../store";
import { useFocusReturn } from "../hooks/useFocusReturn";
import { isLetterKey, isNativeActivationTarget, rovingIndexForKey } from "./modalKeyboard";
import { keepTail, requestCloseTab, tabDetail, tabTitle } from "./tabTitles";
import type { PanelSlot, PanelState, Source, TabKind } from "../ws/types";
import type { WsClient } from "../ws/client";

/** Detail lines keep their tail; CSS clips whatever is still too wide. */
const MAX_DETAIL_CHARS = 60;
/** Fallback page size for PageUp/PageDown when rows cannot be measured. */
const FALLBACK_PAGE_ROWS = 5;

export interface MatchRun {
  text: string;
  matched: boolean;
}

/**
 * Case-insensitive match of `query` against `text`: the contiguous substring
 * when there is one, otherwise a fuzzy subsequence, so `dsc` still finds
 * "Documents". Returns the runs to render, or null when nothing matches.
 */
export function matchRuns(text: string, query: string): MatchRun[] | null {
  const needle = query.trim().toLowerCase();
  if (!needle) return null;
  const hay = text.toLowerCase();
  const start = hay.indexOf(needle);
  let marks: number[];
  if (start >= 0) {
    marks = Array.from({ length: needle.length }, (_, offset) => start + offset);
  } else {
    marks = [];
    let cursor = 0;
    for (const character of needle) {
      const at = hay.indexOf(character, cursor);
      if (at < 0) return null;
      marks.push(at);
      cursor = at + 1;
    }
  }
  const marked = new Set(marks);
  const runs: MatchRun[] = [];
  let buffer = "";
  let bufferMatched = marked.has(0);
  for (let index = 0; index < text.length; index++) {
    const matched = marked.has(index);
    if (matched !== bufferMatched) {
      if (buffer) runs.push({ text: buffer, matched: bufferMatched });
      buffer = "";
      bufferMatched = matched;
    }
    buffer += text[index];
  }
  if (buffer) runs.push({ text: buffer, matched: bufferMatched });
  return runs;
}

/** The panel id of the row whose × holds focus, or null when no × does. */
function closeButtonFocusId(target: EventTarget | null): string | null {
  const panelId = target instanceof HTMLElement ? target.dataset.tabListClose : undefined;
  return panelId ?? null;
}

interface TabRow {
  panelId: string;
  kind: TabKind;
  title: string;
  detail: string;
  /** The slot's own active tab, marked with an accent border. */
  active: boolean;
  closable: boolean;
  editorSessionId?: number;
  titleRuns: MatchRun[];
}

interface Props {
  panelId: string;
  slot: PanelSlot;
  panels: Record<string, PanelState>;
  sources: Record<string, Source>;
  client: WsClient;
}

/**
 * Searchable vertical tab list for one slot, in the style of the sort
 * selector: an overlay inside the panel, dropping from the tab strip. Enter
 * switches to the highlighted tab, Delete closes it, Right arms that row's ×;
 * Left, a list arrow, or the close itself hands focus back to the search
 * field. Escape changes nothing. It is not a
 * `ModalLayer`: the panel keeps input ownership, and the panel handler stays
 * inert while the list is open.
 */
export function PanelTabList({ panelId, slot, panels, sources, client }: Props) {
  const dispatch = useAppDispatch();
  const rootRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const returnFocusRef = useFocusReturn(panelId);
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState(0);
  useSyncExternalStore(subscribeEditorStatus, editorStatusVersion);

  const close = useCallback(() => dispatch({ type: "CLOSE_TAB_LIST" }), [dispatch]);

  const rows = useMemo<TabRow[]>(() => {
    const needle = query.trim();
    return slot.tabPanelIds.reduce<TabRow[]>((list, tabPanelId) => {
      const panel = panels[tabPanelId];
      if (!panel) return list;
      const title = tabTitle(panel, sources);
      const detail = keepTail(tabDetail(panel, sources), MAX_DETAIL_CHARS);
      const titleMatch = needle ? matchRuns(title, needle) : null;
      const detailMatch = needle ? matchRuns(detail, needle) : null;
      if (needle && !titleMatch && !detailMatch) return list;
      list.push({
        panelId: tabPanelId,
        kind: panel.tabKind,
        title,
        detail,
        active: tabPanelId === slot.activePanelId,
        closable: slot.tabPanelIds.length > 1,
        editorSessionId: panel.editor?.sessionId,
        titleRuns: titleMatch ?? [{ text: title, matched: false }],
      });
      return list;
    }, []);
  }, [panels, query, slot.activePanelId, slot.tabPanelIds, sources]);

  // Every query change starts at the first match; a shrinking list keeps the
  // highlight on a row that still exists.
  useEffect(() => {
    setHighlight(0);
  }, [query]);

  useEffect(() => {
    setHighlight((current) => Math.min(current, Math.max(0, rows.length - 1)));
  }, [rows.length]);

  useEffect(() => {
    const selected = listRef.current?.querySelector<HTMLElement>("[data-selected='true']");
    selected?.scrollIntoView({ block: "nearest" });
  }, [highlight, rows]);

  const activate = useCallback((tabPanelId: string) => {
    if (tabPanelId !== slot.activePanelId) {
      client.sendIntent("panel.tab.switch", undefined, {
        slotId: slot.slotId,
        panelId: tabPanelId,
      });
    }
    close();
  }, [client, close, slot.activePanelId, slot.slotId]);

  /** The × of one row, found by row id rather than by position, so a filtered
   *  list still arms the right one. */
  const focusCloseButton = useCallback((tabPanelId: string | undefined) => {
    if (!tabPanelId) return false;
    const button = Array.from(
      listRef.current?.querySelectorAll<HTMLElement>("[data-tab-list-close]") ?? [],
    ).find((element) => element.dataset.tabListClose === tabPanelId);
    if (!button) return false;
    button.focus({ preventScroll: true });
    return true;
  }, []);

  /** The search field is the widget's one Tab stop and the element that
   *  announces the highlighted row, so it is where focus goes whenever an
   *  armed × has to let go: a list arrow, Left, or a closed row. */
  const releaseToField = useCallback(() => {
    const field = inputRef.current;
    if (!field) return;
    field.focus({ preventScroll: true });
    field.setSelectionRange(field.value.length, field.value.length);
  }, []);

  const closeRow = useCallback((tabPanelId: string) => {
    // An × reached by keyboard must not leave focus on the row that is about
    // to disappear, and no other row's × replaces it: the field takes it back.
    const armed = closeButtonFocusId(document.activeElement) === tabPanelId;
    // Editor tabs flush first; the last tab of the slot is refused.
    requestCloseTab(slot, panels, client, tabPanelId);
    // The backend patch lands later; clamp now so no row looks half-selected.
    setHighlight((current) => Math.min(current, Math.max(0, slot.tabPanelIds.length - 2)));
    if (armed) releaseToField();
  }, [client, releaseToField, panels, slot]);

  const pageRows = useCallback(() => {
    const list = listRef.current;
    const height = list?.querySelector<HTMLElement>("[role='option']")?.offsetHeight ?? 0;
    if (!list || !height) return FALLBACK_PAGE_ROWS;
    return Math.max(1, Math.floor(list.clientHeight / height));
  }, []);

  const onKeyDown = useCallback((event: React.KeyboardEvent) => {
    // The list owns the keyboard while it is open: keys typed here must not
    // reach the panel handler or the global shortcut listener.
    event.stopPropagation();
    if (event.defaultPrevented) return;

    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }

    if (event.key === "Delete" || ((event.ctrlKey || event.metaKey) && isLetterKey(event, "w"))) {
      const row = rows[highlight];
      if (!row) return;
      event.preventDefault();
      closeRow(row.panelId);
      return;
    }

    // Which row's × holds focus, if any. The list keeps one Tab stop, so the
    // button is taken with Right and given back by Left, by a list arrow, or
    // by the close itself — never by Tab.
    const armedOn = closeButtonFocusId(event.target);

    if (event.key === "ArrowRight") {
      if (rows.length === 0) return;
      if (armedOn !== null) {
        // Already on an ×: Right stays put. Reaching another row's × means
        // moving the cursor first, which hands this × back to the field.
        event.preventDefault();
        return;
      }
      const field = inputRef.current;
      // A caret still inside the query belongs to the field.
      if (field && field.selectionStart !== null && field.selectionStart < field.value.length) return;
      event.preventDefault();
      focusCloseButton(rows[highlight]?.panelId);
      return;
    }

    if (event.key === "ArrowLeft") {
      if (armedOn === null) return;
      event.preventDefault();
      releaseToField();
      return;
    }

    if (
      armedOn !== null && event.key.length === 1 && event.key !== " "
      && !event.ctrlKey && !event.altKey && !event.metaKey
    ) {
      // Typing continues while an × is armed: the field stays the writer, and
      // Space is left to the focused button, which activates on it.
      const field = inputRef.current;
      if (!field) return;
      event.preventDefault();
      releaseToField();
      setQuery((current) => current + event.key);
      return;
    }

    if (event.key === "PageUp" || event.key === "PageDown") {
      if (rows.length === 0) return;
      event.preventDefault();
      const delta = (event.key === "PageDown" ? 1 : -1) * pageRows();
      const next = Math.min(rows.length - 1, Math.max(0, highlight + delta));
      setHighlight(next);
      // A page move is list navigation, so an armed × lets go.
      if (armedOn !== null) releaseToField();
      return;
    }

    const next = rovingIndexForKey(event.key, highlight, rows.length, "vertical");
    if (next !== null) {
      event.preventDefault();
      setHighlight(next);
      // The cursor moves, the × does not follow it: focus goes back to the
      // field, which is what announces the highlighted row.
      if (armedOn !== null) releaseToField();
      return;
    }

    if (event.key === "Tab") {
      // One Tab stop for the whole widget: leaving the search field closes it.
      close();
      return;
    }

    if (event.key === "Enter") {
      // A row's × is a real button: let it activate on its own, never switch.
      if (isNativeActivationTarget(event.target) && event.target !== inputRef.current) return;
      const row = rows[highlight];
      if (!row) return;
      event.preventDefault();
      activate(row.panelId);
    }
  }, [activate, close, closeRow, focusCloseButton, highlight, pageRows, releaseToField, rows]);

  // Capture the opener on DOM mount and hand focus back on every close path:
  // Escape, outside click, Tab out, a switched or closed tab, unmount.
  const setRootRef = useCallback((node: HTMLDivElement | null) => {
    rootRef.current = node;
    const cleanup = returnFocusRef(node);
    if (node) inputRef.current?.focus({ preventScroll: true });
    return cleanup;
  }, [returnFocusRef]);

  useEffect(() => {
    const onPointerDown = (event: MouseEvent) => {
      const root = rootRef.current;
      if (!root || root.contains(event.target as Node)) return;
      // The count button toggles itself from the strip; do not fight it here.
      if ((event.target as Element).closest?.("[data-tab-list-count]")) return;
      close();
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [close]);

  const listId = `panel-tab-list-options-${panelId}`;
  const selectedRow = rows[highlight];

  return (
    <div className="panel__tab-list-anchor">
      <div
        className="panel__tab-list"
        ref={setRootRef}
        onKeyDown={onKeyDown}
        role="group"
        aria-label="Tabs"
        data-testid="panel-tab-list"
        data-panel-id={panelId}
      >
        <div className="panel__tab-list-search">
          <IconSearch className="panel__tab-list-search-icon" size={14} aria-hidden="true" />
          <input
            ref={inputRef}
            className="panel__tab-list-input"
            data-testid="panel-tab-list-input"
            type="text"
            role="combobox"
            aria-label="Search tabs"
            aria-autocomplete="list"
            aria-expanded="true"
            aria-controls={listId}
            aria-activedescendant={selectedRow ? `${listId}-option-${highlight}` : undefined}
            placeholder="Search tabs…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
          />
        </div>
        <div
          className="panel__tab-list-rows"
          id={listId}
          ref={listRef}
          role="listbox"
          aria-label="Tabs"
        >
          {rows.map((row, index) => {
            const selected = index === highlight;
            const status = row.editorSessionId === undefined ? null : editorStatus(row.editorSessionId);
            return (
              <div
                key={row.panelId}
                id={`${listId}-option-${index}`}
                role="option"
                aria-selected={selected}
                data-selected={selected ? "true" : "false"}
                data-testid={`panel-tab-list-row-${row.panelId}`}
                className={[
                  "panel__tab-list-row",
                  selected ? "panel__tab-list-row--selected" : "",
                  row.active ? "panel__tab-list-row--active" : "",
                ].filter(Boolean).join(" ")}
                title={row.detail || row.title}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => activate(row.panelId)}
              >
                {row.kind === "terminal" && (
                  <IconTerminal2 className="panel__tab-list-icon" size={15} aria-hidden="true" />
                )}
                {row.kind === "editor" && (
                  <IconPencil className="panel__tab-list-icon" size={14} aria-hidden="true" />
                )}
                <span className="panel__tab-list-title">
                  {row.titleRuns.map((run, runIndex) => (
                    run.matched
                      ? <span key={runIndex} className="panel__tab-list-match">{run.text}</span>
                      : <span key={runIndex}>{run.text}</span>
                  ))}
                </span>
                {row.detail && <span className="panel__tab-list-detail">{row.detail}</span>}
                {status?.problem ? (
                  <IconAlertTriangle
                    className="panel__tab-list-warning"
                    size={12}
                    aria-label={status.problem === "error" ? "Save failed" : "Autosave paused"}
                    data-testid={`tab-list-editor-warning-${row.panelId}`}
                  />
                ) : status?.pending ? (
                  <span
                    className="panel__tab-list-dot"
                    aria-label="Saving"
                    data-testid={`tab-list-editor-pending-${row.panelId}`}
                  />
                ) : null}
                {row.closable && (
                  <button
                    className="panel__tab-list-close"
                    type="button"
                    tabIndex={-1}
                    data-tab-list-close={row.panelId}
                    title="Close tab"
                    aria-label={`Close ${row.title} tab`}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={(event) => {
                      event.stopPropagation();
                      closeRow(row.panelId);
                    }}
                  >
                    <IconX size={11} />
                  </button>
                )}
              </div>
            );
          })}
          {rows.length === 0 && (
            <div className="panel__tab-list-empty" role="status">No matching tabs</div>
          )}
        </div>
      </div>
    </div>
  );
}
