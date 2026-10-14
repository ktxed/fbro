import { useEffect, useMemo, useRef, useState } from "react";

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
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => executeSelectedCommand(command.id)}
                >
                  <span className="command-palette__item-main">
                    <span className="command-palette__item-title">{command.title}</span>
                    <span className="command-palette__item-category">{command.category}</span>
                  </span>
                  {binding && (
                    <span className="command-palette__item-binding">{binding}</span>
                  )}
                  {command.id === "app.about" && (
                    <img className="command-palette__item-icon" src={fbroIcon} alt="" aria-hidden="true" />
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
