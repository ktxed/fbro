import { useEffect, useMemo, useRef, useState } from "react";
import {
  IconSearch, IconSettings, IconListDetails, IconLayoutBottombar, IconTerminal2,
  IconDeviceUsb, IconTerminal, IconLogout, IconInfoCircle, IconPencil, IconEye,
  IconEdit, IconFilePlus, IconCopy, IconArrowsMove, IconCut, IconClipboard,
  IconFolderPlus, IconTrash, IconFiles, IconLock, IconHome, IconFolder, IconRefresh,
  IconHistory, IconFilter, IconArrowsExchange, IconDots, IconEyeOff, IconLayoutGrid,
  IconArrowsSort, IconGitBranch, IconBookmark, IconBookmarks, IconPlus, IconX,
  IconRobot, IconLayoutSidebar, IconArrowUp, IconArrowDown, IconArrowLeft,
  IconArrowRight, IconArrowBarToUp, IconArrowBarToDown, IconChevronsUp,
  IconChevronsDown, IconChevronLeft, IconChevronRight, IconArrowsLeftRight,
  IconFolderOpen, IconCheckbox, IconCalculator, IconSelectAll, IconSquareOff,
  IconCommand,
} from "@tabler/icons-react";

import {
  COMMAND_PALETTE_COMMAND_IDS,
  executeCommand,
  formatPrimaryBinding,
  getCommand,
} from "../commands/interactive";
import { useAppDispatch, useAppState } from "../store";
import type { WsClient } from "../ws/client";
import type { InteractiveCommand } from "../ws/types";
import { ResizableModal } from "./ResizableModal";
import { ModalLayer } from "./ModalStack";
import { isNativeActivationTarget } from "./modalKeyboard";

const fbroIcon = new URL("../../logo256.png", import.meta.url).href;

// Icons are visual cues; related navigation and selection commands deliberately
// reuse directional/action icons while their labels describe the exact action.
const COMMAND_ICONS: Partial<Record<string, typeof IconSearch>> = {
  "app.search": IconSearch,
  "app.settings": IconSettings,
  "app.open_jobs": IconListDetails,
  "app.toggle_button_bar": IconLayoutBottombar,
  "app.install_fbro_command": IconTerminal2,
  "app.eject_source": IconDeviceUsb,
  "app.open_terminal": IconTerminal,
  "app.quit": IconLogout,
  "file.info": IconInfoCircle,
  "file.rename": IconPencil,
  "file.view": IconEye,
  "file.edit": IconEdit,
  "file.quick_edit": IconEdit,
  "file.touch": IconFilePlus,
  "file.copy": IconCopy,
  "file.copy_external": IconCopy,
  "file.move": IconArrowsMove,
  "file.cut_external": IconCut,
  "file.paste_external": IconClipboard,
  "file.paste_external_move": IconClipboard,
  "file.mkdir": IconFolderPlus,
  "file.delete": IconTrash,
  "file.compare": IconFiles,
  "file.set_permissions": IconLock,
  "panel.navigate_home": IconHome,
  "panel.navigate_to_path": IconFolder,
  "panel.navigate_source_root": IconFolder,
  "panel.refresh": IconRefresh,
  "panel.history.open": IconHistory,
  "panel.filter.open": IconFilter,
  "panel.path.sync": IconArrowsExchange,
  "panel.context_menu": IconDots,
  "panel.toggle_hidden": IconEyeOff,
  "panel.toggle_view_mode": IconLayoutGrid,
  "panel.sort.open": IconArrowsSort,
  "panel.git.actions": IconGitBranch,
  "git.init": IconGitBranch,
  "panel.bookmarks.open": IconBookmark,
  "panel.bookmarks.browse": IconBookmarks,
  "panel.toggle_embedded_terminal": IconTerminal,
  "panel.tab.new": IconPlus,
  "panel.tab.close": IconX,
  "panel.pass_to_bro": IconRobot,
  "panel.viewer.embed": IconLayoutSidebar,
  "panel.cursor.up": IconArrowUp,
  "panel.cursor.down": IconArrowDown,
  "panel.cursor.left": IconArrowLeft,
  "panel.cursor.right": IconArrowRight,
  "panel.cursor.first": IconArrowBarToUp,
  "panel.cursor.last": IconArrowBarToDown,
  "panel.cursor.page_up": IconChevronsUp,
  "panel.cursor.page_down": IconChevronsDown,
  "panel.selection.extend_up": IconArrowUp,
  "panel.selection.extend_down": IconArrowDown,
  "panel.selection.extend_left": IconArrowLeft,
  "panel.selection.extend_right": IconArrowRight,
  "panel.selection.extend_first": IconArrowBarToUp,
  "panel.selection.extend_last": IconArrowBarToDown,
  "panel.selection.toggle_or_size": IconCheckbox,
  "panel.selection.calculate_sizes": IconCalculator,
  "panel.selection.select_all": IconSelectAll,
  "panel.selection.clear": IconSquareOff,
  "panel.selection.invert": IconArrowsExchange,
  "panel.selection.select_by_mask": IconFilter,
  "panel.selection.deselect_by_mask": IconFilter,
  "panel.open": IconFolderOpen,
  "panel.open_entry": IconFolderOpen,
  "panel.navigate_up": IconArrowUp,
  "panel.history.back": IconArrowLeft,
  "panel.history.forward": IconArrowRight,
  "panel.focus.switch": IconArrowsLeftRight,
  "panel.source.next": IconChevronRight,
  "panel.source.previous": IconChevronLeft,
  "panel.tab.next": IconChevronRight,
  "panel.tab.prev": IconChevronLeft,
};

interface CommandPaletteModalProps {
  client: WsClient;
  onOpenJobsView: () => void;
}

export function CommandPaletteModal({ client, onOpenJobsView }: CommandPaletteModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);

  const commands = useMemo(() => (
    COMMAND_PALETTE_COMMAND_IDS
      .map((commandId) => getCommand(state.commands, commandId))
      .filter((command): command is InteractiveCommand => !!command)
  ), [state.commands]);

  const filteredCommands = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase();
    if (!normalizedQuery) return commands;

    return commands.filter((command) => {
      const haystack = [
        command.id,
        command.category,
        command.title,
        formatPrimaryBinding(command),
      ].join(" ").toLowerCase();
      return haystack.includes(normalizedQuery);
    });
  }, [commands, query]);

  useEffect(() => {
    if (filteredCommands.length === 0) {
      setSelectedIndex(0);
      return;
    }
    setSelectedIndex((current) => Math.min(current, filteredCommands.length - 1));
  }, [filteredCommands]);

  useEffect(() => {
    const selected = listRef.current?.querySelector<HTMLElement>("[data-selected='true']");
    selected?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex, filteredCommands]);

  const close = () => dispatch({ type: "CLOSE_COMMAND_PALETTE" });

  const executeSelectedCommand = (commandId: string) => {
    close();
    executeCommand(commandId, {
      state,
      dispatch,
      client,
      onOpenJobsView,
      onOpenCommandPalette: () => dispatch({ type: "OPEN_COMMAND_PALETTE" }),
      onOpenContextMenu: () => dispatch({ type: "CLOSE_COMMAND_PALETTE" }),
    });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.defaultPrevented) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }

    if (isNativeActivationTarget(e.target) && e.target !== inputRef.current) return;

    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (filteredCommands.length > 0) {
        setSelectedIndex((current) => (current + 1) % filteredCommands.length);
      }
      return;
    }

    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (filteredCommands.length > 0) {
        setSelectedIndex((current) => (current - 1 + filteredCommands.length) % filteredCommands.length);
      }
      return;
    }

    if (e.key === "Enter") {
      e.preventDefault();
      const command = filteredCommands[selectedIndex];
      if (command) executeSelectedCommand(command.id);
    }
  };

  return (
    <ModalLayer modalId="command-palette" initialFocusRef={inputRef} onClick={close}>
      <ResizableModal
        modalId="command-palette"
        client={client}
        className="command-palette"
        ariaModal
        ariaLabel="Command Palette"
        onKeyDown={onKeyDown}
        defaultSize={{ width: 560, height: 420 }}
        minSize={{ width: 420, height: 300 }}
      >
        <div className="modal__header">Command Palette</div>
        <div className="modal__body command-palette__body">
          <input
            ref={inputRef}
            className="command-palette__input"
            role="combobox"
            aria-autocomplete="list"
            aria-expanded="true"
            aria-controls="command-palette-list"
            aria-activedescendant={filteredCommands[selectedIndex] ? `command-palette-option-${selectedIndex}` : undefined}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search commands…"
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
          />
          <div id="command-palette-list" className="command-palette__list" ref={listRef} role="listbox">
            {filteredCommands.map((command, index) => {
              const binding = formatPrimaryBinding(command);
              const CommandIcon = COMMAND_ICONS[command.id] ?? IconCommand;
              const selected = index === selectedIndex;
              return (
                <button
                  key={command.id}
                  id={`command-palette-option-${index}`}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  tabIndex={-1}
                  className={`command-palette__item${selected ? " command-palette__item--selected" : ""}`}
                  data-selected={selected ? "true" : "false"}
                  data-command-id={command.id}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => executeSelectedCommand(command.id)}
                >
                  <span className="command-palette__item-icon" aria-hidden="true">
                    {command.id === "app.about" ? <img src={fbroIcon} alt="" />
                      : <CommandIcon size={20} />}
                  </span>
                  <span className="command-palette__item-main">
                    <span className="command-palette__item-title">{command.title}</span>
                    <span className="command-palette__item-category">{command.category}</span>
                  </span>
                  {binding && (
                    <span className="command-palette__item-binding">{binding}</span>
                  )}
                </button>
              );
            })}
            {filteredCommands.length === 0 && (
              <div className="command-palette__empty">No matching commands.</div>
            )}
          </div>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
