import {
  autoUpdate,
  flip,
  FloatingPortal,
  offset,
  shift,
  size,
  useFloating,
} from "@floating-ui/react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { bindingMatches, executeTargetedCommand } from "../commands/interactive";
import { buildContextMenuItems } from "../contextMenu";
import { useAppDispatch, useAppState, type AppState, type PendingContextMenu } from "../store";
import { resolveColorScheme } from "../theme/colorSchemes";
import { isGlobalSelectionSourceId } from "../utils/sourceId";
import type { WsClient } from "../ws/client";
import type { PanelState, VFSEntry } from "../ws/types";

interface ContextMenuProps {
  client: WsClient;
  menu: PendingContextMenu;
}

function entryAt(panel: PanelState, absIndex: number): VFSEntry | undefined {
  return panel.entries[absIndex - (panel.windowStart ?? 0)];
}

function cursorFile(panel?: PanelState): VFSEntry | undefined {
  if (!panel) return undefined;
  const entry = entryAt(panel, panel.cursor);
  return entry?.kind === "file" && entry.name !== ".." ? entry : undefined;
}

function selectedFiles(panel?: PanelState): VFSEntry[] {
  if (!panel) return [];
  return panel.selection
    .map((index) => entryAt(panel, index))
    .filter((entry): entry is VFSEntry => !!entry && entry.kind === "file" && entry.name !== "..");
}

function canCompareFromMenu(state: AppState, menu: PendingContextMenu) {
  if (menu.kind !== "file") return false;
  const panel = state.panels[menu.panelId];
  if (!panel) return false;
  const currentSelected = selectedFiles(panel);
  if (currentSelected.length === 2) {
    return menu.targetScope === "selection" || currentSelected.some((entry) => entry.name === menu.name);
  }
  const targetName = menu.name;
  if (currentSelected.length === 1) {
    const cursor = cursorFile(panel);
    if (cursor && cursor.name !== currentSelected[0].name) return true;
    if (targetName !== currentSelected[0].name) return true;
  }

  const panelIds = state.layout.panels.length > 0 ? state.layout.panels : Object.keys(state.panels);
  const otherPanel = state.panels[panelIds.find((id) => id !== menu.panelId) ?? ""];
  const thisCandidates = currentSelected.length === 1 ? currentSelected : [{ name: targetName, kind: "file" } as VFSEntry];
  const otherSelected = selectedFiles(otherPanel);
  const otherCandidates = otherSelected.length === 1 ? otherSelected : otherSelected.length === 0 ? [cursorFile(otherPanel)].filter(Boolean) as VFSEntry[] : [];
  return thisCandidates.length === 1 && otherCandidates.length === 1;
}

export function ContextMenu({ client, menu }: ContextMenuProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const menuRef = useRef<HTMLDivElement>(null);
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const colorScheme = resolveColorScheme(state.settings.general.colorScheme);

  const items = useMemo(
    () => {
      const panel = state.panels[menu.panelId];
      const source = panel ? state.sources[panel.sourceId] : undefined;
      const isLocalFilesystem = !!source
        && source.kind === "filesystem"
        && !menu.isShadow
        && !source.shadow
        && source.mounted;
      const isGlobalSelection = !!panel && isGlobalSelectionSourceId(panel.sourceId);
      const isGlobalRoot = isGlobalSelection && panel.path === "";
      const isGlobalInside = isGlobalSelection && panel.path !== "";
      const supports = source?.supports ?? [];
      return buildContextMenuItems(
        state.commands,
        menu.fullPath,
        menu.name,
        menu.kind,
        {
          isShadow: menu.isShadow,
          canPlatform: isLocalFilesystem,
          canOpenWith: isLocalFilesystem,
          canPassToBro: isLocalFilesystem || isGlobalRoot || canCompareFromMenu(state, menu),
          canCompare: canCompareFromMenu(state, menu),
          canAddToGlobalSelection: isLocalFilesystem,
          canCopy: !!menu.isShadow || (isLocalFilesystem && supports.includes("CopySelection")),
          canRename: (isLocalFilesystem && supports.includes("Rename")) || isGlobalRoot,
          canDelete: (isLocalFilesystem && supports.includes("DeleteSelection")) || isGlobalSelection,
          canCopyFullPath: isLocalFilesystem || isGlobalInside,
        },
      );
    },
    [menu.fullPath, menu.kind, menu.name, menu.isShadow, menu.panelId, state.commands, state.panels, state.sources],
  );
  const selectableItems = items.filter((item) => item.kind !== "separator");

  const close = () => dispatch({ type: "CLOSE_CONTEXT_MENU" });

  const virtualReference = useMemo(
    () => ({
      getBoundingClientRect: () => DOMRect.fromRect({
        x: menu.anchorLeft,
        y: menu.anchorTop,
        width: menu.anchorRight - menu.anchorLeft,
        height: menu.anchorBottom - menu.anchorTop,
      }),
    }),
    [menu.anchorBottom, menu.anchorLeft, menu.anchorRight, menu.anchorTop],
  );

  const { refs, floatingStyles } = useFloating({
    open: true,
    placement: "bottom-start",
    strategy: "fixed",
    whileElementsMounted: autoUpdate,
    middleware: [
      offset(4),
      flip({
        padding: 8,
        fallbackPlacements: ["top-start", "bottom-end", "top-end"],
      }),
      shift({ padding: 8 }),
      size({
        padding: 8,
        apply({ availableWidth, availableHeight, elements }) {
          Object.assign(elements.floating.style, {
            maxWidth: `${Math.max(160, Math.floor(availableWidth))}px`,
            maxHeight: `${Math.max(120, Math.floor(availableHeight))}px`,
          });
        },
      }),
    ],
  });

  useLayoutEffect(() => {
    refs.setReference(virtualReference);
  }, [refs, virtualReference]);

  useEffect(() => {
    setHighlightedIndex(items.findIndex((item) => item.kind !== "separator"));
  }, [items]);

  useLayoutEffect(() => {
    const focusMenu = () => menuRef.current?.focus();
    focusMenu();
    const rafId = window.requestAnimationFrame(focusMenu);
    return () => window.cancelAnimationFrame(rafId);
  }, []);

  useEffect(() => {
    const panel = state.panels[menu.panelId];
    if (!panel || panel.path.replace(/[/\\]+$/, "") !== menu.dirPath) {
      close();
      return;
    }
    const entry = panel.entries[menu.absIndex - (panel.windowStart ?? 0)];
    if (!entry || entry.name !== menu.name) {
      close();
    }
  }, [close, menu.absIndex, menu.dirPath, menu.name, menu.panelId, state.panels]);

  const runItem = async (index: number) => {
    const item = items[index];
    if (!item || item.kind === "separator") return;

    close();

    if (item.kind === "command" && item.commandId) {
      executeTargetedCommand(
        item.commandId,
        { state, dispatch, client },
        {
          panelId: menu.panelId,
          dirPath: menu.dirPath,
          fullPath: menu.fullPath,
          name: menu.name,
          kind: menu.kind,
          size: menu.size,
        },
      );
      return;
    }

    if (item.kind === "clipboard" && item.clipboardText) {
      await navigator.clipboard.writeText(item.clipboardText);
      return;
    }

    if (item.kind === "openWith") {
      const requestId = globalThis.crypto?.randomUUID?.() ?? `open-with-${Date.now()}-${Math.random().toString(16).slice(2)}`;
      dispatch({
        type: "OPEN_OPEN_WITH",
        openWith: {
          panelId: menu.panelId,
          absIndex: menu.absIndex,
          name: menu.name,
          kind: menu.kind,
          dirPath: menu.dirPath,
          fullPath: menu.fullPath,
          requestId,
          status: "loading",
          apps: [],
        },
      });
      client.sendIntent("ListOpenWithApps", undefined, {
        requestId,
        path: menu.fullPath,
        kind: menu.kind,
      });
      return;
    }

    if (item.kind === "bro") {
      const requestId = globalThis.crypto?.randomUUID?.() ?? `bro-${Date.now()}-${Math.random().toString(16).slice(2)}`;
      const otherPanelId = (state.layout.panels.length > 0 ? state.layout.panels : Object.keys(state.panels))
        .find((panelId) => panelId !== menu.panelId);
      const suggestedOutputDir = otherPanelId ? state.panels[otherPanelId]?.path : undefined;
      dispatch({
        type: "OPEN_BRO",
        bro: {
          panelId: menu.panelId,
          dirPath: menu.dirPath,
          requestId,
          explicitPath: menu.fullPath,
          status: "loading",
          inputs: [],
          bros: [],
        },
      });
      client.sendIntent("ListBros", menu.panelId, {
        requestId,
        targetPath: menu.fullPath,
        suggestedOutputDir,
      });
      return;
    }

    if (item.kind === "globalSelection") {
      const targetPath = menu.targetScope === "selection" ? undefined : menu.fullPath;
      const targetName = menu.targetScope === "selection"
        ? `${menu.targetCount} selected ${menu.targetCount === 1 ? "entry" : "entries"}`
        : menu.name;
      if (state.globalSelections.length === 0) {
        client.sendIntent("AddToGlobalSelection", menu.panelId, {
          panelId: menu.panelId,
          ...(targetPath ? { targetPath } : {}),
        });
      } else {
        dispatch({
          type: "OPEN_GLOBAL_SELECTION_ADD",
          add: {
            panelId: menu.panelId,
            ...(targetPath ? { targetPath } : {}),
            targetName,
            targetCount: menu.targetCount,
          },
        });
      }
      return;
    }

    if (item.kind === "platform" && item.actionId) {
      client.sendIntent("RevealInFileManager", undefined, {
        path: menu.fullPath,
        kind: menu.kind,
        actionId: item.actionId,
      });
    }
  };

  const moveHighlight = (delta: number) => {
    const current = selectableItems.findIndex((item) => item.id === items[highlightedIndex]?.id);
    const nextSelectable = selectableItems[(current + delta + selectableItems.length) % selectableItems.length];
    const nextIndex = items.findIndex((item) => item.id === nextSelectable.id);
    if (nextIndex >= 0) setHighlightedIndex(nextIndex);
  };

  const handleMenuKeyDown = (event: Pick<KeyboardEvent, "key" | "shiftKey" | "ctrlKey" | "altKey" | "metaKey" | "preventDefault">) => {
    if (event.key === "Tab") {
      event.preventDefault();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      moveHighlight(1);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      moveHighlight(-1);
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      void runItem(highlightedIndex);
      return;
    }

    const shortcutIndex = items.findIndex((item) => {
      if (item.kind !== "command" || !item.commandId) return false;
      const command = state.commands[item.commandId];
      return !!command && command.bindings.some((binding) => bindingMatches(event as KeyboardEvent, binding));
    });
    if (shortcutIndex >= 0) {
      event.preventDefault();
      void runItem(shortcutIndex);
      return;
    }

    event.preventDefault();
  };

  useEffect(() => {
    const onWindowKeyDown = (event: KeyboardEvent) => {
      handleMenuKeyDown(event);
    };

    window.addEventListener("keydown", onWindowKeyDown, true);
    return () => window.removeEventListener("keydown", onWindowKeyDown, true);
  }, [highlightedIndex, items, state.commands]);

  return (
    <FloatingPortal>
      <div
        className="context-menu__overlay"
        data-color-scheme={colorScheme.id}
        style={{ colorScheme: colorScheme.mode }}
        onMouseDown={close}
        onContextMenu={(e) => e.preventDefault()}
      >
        <div
          className="context-menu"
          ref={(node) => {
            menuRef.current = node;
            refs.setFloating(node);
          }}
          role="menu"
          tabIndex={-1}
          style={floatingStyles}
          onMouseDown={(e) => e.stopPropagation()}
        >
          {items.map((item, index) => {
            if (item.kind === "separator") {
              return <div key={item.id} className="context-menu__separator" />;
            }
            return (
              <button
                key={item.id}
                className={`context-menu__item${index === highlightedIndex ? " context-menu__item--active" : ""}`}
                data-command-id={item.kind === "command" && item.commandId ? item.commandId : undefined}
                role="menuitem"
                onMouseEnter={() => setHighlightedIndex(index)}
                onClick={() => { void runItem(index); }}
              >
                <span>{item.label}</span>
                <span className="context-menu__shortcut">{item.shortcut ?? ""}</span>
              </button>
            );
          })}
        </div>
      </div>
    </FloatingPortal>
  );
}
