import {
  autoUpdate,
  flip,
  FloatingPortal,
  offset,
  shift,
  size,
  useFloating,
} from "@floating-ui/react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { bindingMatches, executeTargetedCommand } from "../commands/interactive";
import { buildContextMenuItems } from "../contextMenu";
import { useAppDispatch, useAppState, type PendingContextMenu } from "../store";
import { useCopyToClipboard } from "../hooks/useCopyToClipboard";
import { useFocusReturn } from "../hooks/useFocusReturn";
import { handoffFocusToOpener } from "../utils/focusReturn";
import { resolveColorScheme } from "../theme/colorSchemes";
import { contextualOperationAvailable, resolveContextualTarget } from "../contextualOperations";
import type { WsClient } from "../ws/client";

interface ContextMenuProps {
  client: WsClient;
  menu: PendingContextMenu;
}

export function ContextMenu({ client, menu }: ContextMenuProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const copyToClipboard = useCopyToClipboard();
  const menuRef = useRef<HTMLDivElement>(null);
  const returnFocusRef = useFocusReturn(menu.panelId);
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const colorScheme = resolveColorScheme(state.settings.general.colorScheme);

  const items = useMemo(
    () => {
      const panel = state.panels[menu.panelId];
      const target = panel ? resolveContextualTarget(panel, menu.absIndex) : null;
      if (!target || target.primary.fullPath !== menu.fullPath || target.primary.kind !== menu.kind) return [];
      const can = (id: Parameters<typeof contextualOperationAvailable>[0]) => contextualOperationAvailable(id, state, target);
      return buildContextMenuItems(
        state.commands,
        menu.fullPath,
        menu.name,
        menu.kind,
        {
          isShadow: menu.isShadow,
          canPlatform: can("platform.reveal"),
          canOpenWith: can("platform.open-with"),
          canPassToBro: can("panel.pass-to-bro"),
          canCompare: can("file.compare"),
          canGitDiff: can("git.view-diff"),
          canGitInit: can("git.init"),
          canAddToGlobalSelection: can("selection.to-global"),
          canCopy: can("file.copy"),
          canMove: can("file.move"),
          canRename: can("file.rename"),
          canDelete: can("file.delete"),
          canCopyFullPath: can("clipboard.paths"),
          targetCount: target.count,
          clipboardNames: target.entries.map((entry) => entry.name).join("\n"),
          clipboardPaths: target.entries.map((entry) => entry.fullPath).join("\n"),
        },
      );
    },
    [menu.absIndex, menu.fullPath, menu.kind, menu.name, menu.isShadow, menu.panelId, state.commands, state.panels, state.sources],
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

  const setMenuRef = useCallback((node: HTMLDivElement | null) => {
    refs.setFloating(node);
    menuRef.current = node;
    const cleanup = returnFocusRef(node);
    node?.focus({ preventScroll: true });
    return cleanup;
  }, [refs.setFloating, returnFocusRef]);

  useEffect(() => {
    const panel = state.panels[menu.panelId];
    if (!panel || panel.path.replace(/[/\\]+$/, "") !== menu.dirPath) {
      close();
      return;
    }
    const entry = panel.entries[menu.absIndex - (panel.windowStart ?? 0)];
    if (!entry || entry.name !== menu.name || entry.kind !== menu.kind
      || (entry.sourcePath ?? `${menu.dirPath}/${entry.name}`) !== menu.fullPath) {
      close();
    }
  }, [close, menu.absIndex, menu.dirPath, menu.fullPath, menu.kind, menu.name, menu.panelId, state.panels]);

  const runItem = async (index: number) => {
    const item = items[index];
    if (!item || item.kind === "separator") return;

    handoffFocusToOpener(menuRef.current);
    close();
    const panel = state.panels[menu.panelId];
    const target = panel ? resolveContextualTarget(panel, menu.absIndex) : null;
    if (!target || target.primary.fullPath !== menu.fullPath || target.primary.kind !== menu.kind) return;

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
          scope: target.scope,
          entries: target.entries,
        },
      );
      return;
    }

    if (item.kind === "clipboard" && item.clipboardText) {
      const noun = item.id === "clipboard.paths" ? "full path" : "name";
      copyToClipboard(item.clipboardText, target.count === 1 ? noun : `${target.count} ${noun}s`);
      return;
    }

    if (item.kind === "gitDiff") {
      client.sendIntent("GitViewDiff", undefined, {
        panelId: menu.panelId,
        targetPath: menu.fullPath,
      });
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
          ...(target.scope === "entry" ? { explicitPath: target.primary.fullPath } : {}),
          status: "loading",
          inputs: [],
          bros: [],
        },
      });
      client.sendIntent("ListBros", menu.panelId, {
        requestId,
        ...(target.scope === "entry" ? { targetPath: target.primary.fullPath } : {}),
        suggestedOutputDir,
      });
      return;
    }

    if (item.kind === "globalSelection") {
      const targetPath = target.scope === "selection" ? undefined : target.primary.fullPath;
      const targetName = target.scope === "selection"
        ? `${target.count} selected ${target.count === 1 ? "entry" : "entries"}`
        : target.primary.name;
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
            targetCount: target.count,
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
          ref={setMenuRef}
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
                data-contextual-operation={item.id}
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
