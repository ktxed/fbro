import { useRef } from "react";

import { bindingMatches } from "../commands/bindings";
import { CHEAT_SHEET_COMMAND_ID, COMMAND_PALETTE_COMMAND_ID, TOOLBAR_COMMAND_IDS } from "../commands/interactive";
import { useAppDispatch, useAppState } from "../store";
import { isMac } from "../utils/platform";
import type { WsClient } from "../ws/client";
import type { InteractiveCommand, KeyBinding } from "../ws/types";
import { KeyCombo } from "./KeyCombo";
import { ModalLayer } from "./ModalStack";
import { ResizableModal } from "./ResizableModal";

interface CheatSheetRow {
  label: string;
  /** Registry commands whose bindings make up this row. With several ids each
   *  contributes its primary binding; a single id shows up to two bindings. */
  commandIds?: string[];
  /** Fixed, non-registry shortcuts (zoom). */
  fixed?: KeyBinding[];
}

interface CheatSheetSection {
  title: string;
  rows: CheatSheetRow[];
}

const modStroke = (key: string): KeyBinding => ({
  strokes: [{ key, shift: false, ctrl: !isMac, alt: false, meta: isMac }],
  modes: [],
});

// The bottom button bar already advertises its commands (TOOLBAR_COMMAND_IDS),
// so they are left out here; plain arrow/page movement is omitted as obvious.
const SECTIONS: CheatSheetSection[] = [
  {
    title: "Navigation",
    rows: [
      { label: "Switch panel", commandIds: ["panel.focus.switch"] },
      { label: "Open item", commandIds: ["panel.open"] },
      { label: "Parent folder", commandIds: ["panel.navigate_up"] },
      { label: "Back / forward", commandIds: ["panel.history.back", "panel.history.forward"] },
      { label: "Browse history", commandIds: ["panel.history.open"] },
      { label: "First item", commandIds: ["panel.cursor.first"] },
      { label: "Last item", commandIds: ["panel.cursor.last"] },
      { label: "Home folder", commandIds: ["panel.navigate_home"] },
      { label: "Source root", commandIds: ["panel.navigate_source_root"] },
      { label: "Go to path", commandIds: ["panel.navigate_to_path"] },
      { label: "Sync other panel here", commandIds: ["panel.path.sync"] },
      { label: "Next / previous source", commandIds: ["panel.source.next", "panel.source.previous"] },
    ],
  },
  {
    title: "Selection",
    rows: [
      { label: "Toggle item / folder size", commandIds: ["panel.selection.toggle_or_size"] },
      { label: "Extend selection", commandIds: ["panel.selection.extend_up", "panel.selection.extend_down"] },
      { label: "Select all", commandIds: ["panel.selection.select_all"] },
      { label: "Clear selection", commandIds: ["panel.selection.clear"] },
      { label: "Invert selection", commandIds: ["panel.selection.invert"] },
      { label: "Select / deselect by mask", commandIds: ["panel.selection.select_by_mask", "panel.selection.deselect_by_mask"] },
      { label: "Selected folder sizes", commandIds: ["panel.selection.calculate_sizes"] },
    ],
  },
  {
    title: "Panel",
    rows: [
      { label: "Filter", commandIds: ["panel.filter.open"] },
      { label: "Sort", commandIds: ["panel.sort.open"] },
      { label: "Hidden files", commandIds: ["panel.toggle_hidden"] },
      { label: "Grid view", commandIds: ["panel.toggle_view_mode"] },
      { label: "Refresh", commandIds: ["panel.refresh"] },
      { label: "Embedded viewer", commandIds: ["panel.viewer.embed"] },
      { label: "Embedded terminal", commandIds: ["panel.toggle_embedded_terminal"] },
      { label: "Maximize / restore terminal", commandIds: ["panel.terminal.maximize"] },
      { label: "Promote terminal to a tab", commandIds: ["panel.terminal.promote"] },
      { label: "Bookmarks", commandIds: ["panel.bookmarks.open"] },
      { label: "Browse bookmarks", commandIds: ["panel.bookmarks.browse"] },
    ],
  },
  {
    title: "Tabs",
    rows: [
      { label: "New tab", commandIds: ["panel.tab.new"] },
      { label: "Close tab", commandIds: ["panel.tab.close"] },
      { label: "Next / previous tab", commandIds: ["panel.tab.next", "panel.tab.prev"] },
      { label: "All tabs…", commandIds: ["panel.tab.list"] },
    ],
  },
  {
    title: "Files",
    rows: [
      { label: "Inspect", commandIds: ["file.info"] },
      { label: "Pass to Bro", commandIds: ["panel.pass_to_bro"] },
      { label: "Context menu", commandIds: ["panel.context_menu"] },
      { label: "New file", commandIds: ["file.touch"] },
      { label: "Quick edit", commandIds: ["file.quick_edit"] },
      { label: "Copy to clipboard", commandIds: ["file.copy_external"] },
      { label: "Paste from clipboard", commandIds: ["file.paste_external"] },
      { label: "Git", commandIds: ["panel.git.actions"] },
      { label: "New note", commandIds: ["notes.new"] },
    ],
  },
  {
    title: "App",
    rows: [
      { label: "Command palette", commandIds: [COMMAND_PALETTE_COMMAND_ID] },
      { label: "This cheat sheet", commandIds: [CHEAT_SHEET_COMMAND_ID] },
      { label: "Search", commandIds: ["app.search"] },
      { label: "Jobs", commandIds: ["app.open_jobs"] },
      { label: "Settings", commandIds: ["app.settings"] },
      { label: "About fbro", commandIds: ["app.about"] },
      { label: "Zoom in / out / reset", fixed: [modStroke("+"), modStroke("-"), modStroke("0")] },
    ],
  },
];

const TOOLBAR_IDS = new Set<string>(TOOLBAR_COMMAND_IDS);

function rowBindings(row: CheatSheetRow, commands: Record<string, InteractiveCommand>): KeyBinding[] {
  if (row.fixed) return row.fixed;
  const ids = (row.commandIds ?? []).filter((id) => !TOOLBAR_IDS.has(id));
  if (ids.length === 1) return commands[ids[0]]?.bindings.slice(0, 2) ?? [];
  return ids.flatMap((id) => commands[id]?.bindings.slice(0, 1) ?? []);
}

export function CheatSheetModal({ client }: { client: WsClient }) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const bodyRef = useRef<HTMLDivElement>(null);

  const close = () => dispatch({ type: "CLOSE_CHEAT_SHEET" });
  const openSettings = () => {
    close();
    dispatch({ type: "OPEN_SETTINGS", tab: "keyboard" });
  };

  const sheetCommand = state.commands[CHEAT_SHEET_COMMAND_ID];
  const paletteCommand = state.commands[COMMAND_PALETTE_COMMAND_ID];
  const mode = state.panels[state.focus]?.viewMode ?? "list";

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.defaultPrevented) return;
    // Escape or the cheat-sheet shortcut itself closes the sheet.
    if (event.key === "Escape" || sheetCommand?.bindings.some((binding) => bindingMatches(event.nativeEvent, binding, mode))) {
      event.preventDefault();
      close();
    }
  };

  return (
    <ModalLayer modalId="cheat-sheet" initialFocusRef={bodyRef} onClick={close} onKeyDown={onKeyDown}>
      <ResizableModal
        modalId="cheat-sheet"
        client={client}
        className="cheat-sheet"
        ariaModal
        ariaLabel="Keyboard Cheat Sheet"
        testId="cheat-sheet-modal"
        defaultSize={{ width: 900, height: 600 }}
        minSize={{ width: 420, height: 320 }}
      >
        <div className="modal__header">Keyboard Cheat Sheet</div>
        <div
          ref={bodyRef}
          className="modal__body cheat-sheet__body"
          tabIndex={0}
          aria-label="Keyboard shortcuts"
          data-testid="cheat-sheet-body"
        >
          {SECTIONS.map((section) => (
            <section key={section.title} className="cheat-sheet__section" aria-label={section.title}>
              <h3 className="cheat-sheet__section-title">{section.title}</h3>
              <dl className="cheat-sheet__rows">
                {section.rows.map((row) => {
                  const bindings = rowBindings(row, state.commands);
                  return (
                    <div key={row.label} className="cheat-sheet__row" data-command-id={row.commandIds?.[0]}>
                      <dt className="cheat-sheet__label">{row.label}</dt>
                      <dd className="cheat-sheet__keys">
                        {bindings.length > 0
                          ? bindings.map((binding, index) => (
                            <span key={index} className="cheat-sheet__binding">
                              {index > 0 && <span className="cheat-sheet__separator" aria-hidden="true">/</span>}
                              <KeyCombo binding={binding} />
                            </span>
                          ))
                          : (
                            <span className="cheat-sheet__palette-hint">
                              Command palette
                              {paletteCommand?.bindings[0] && <KeyCombo binding={paletteCommand.bindings[0]} />}
                            </span>
                          )}
                      </dd>
                    </div>
                  );
                })}
              </dl>
            </section>
          ))}
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" type="button" onClick={close} data-testid="cheat-sheet-close">
            Close
          </button>
          <button className="modal__btn" type="button" onClick={openSettings} data-testid="cheat-sheet-customize">
            Customize…
          </button>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
