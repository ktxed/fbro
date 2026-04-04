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
import { useAppDispatch, useAppState, type PendingContextMenu } from "../store";
import type { WsClient } from "../ws/client";

interface ContextMenuProps {
  client: WsClient;
  menu: PendingContextMenu;
}

export function ContextMenu({ client, menu }: ContextMenuProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const menuRef = useRef<HTMLDivElement>(null);
  const [highlightedIndex, setHighlightedIndex] = useState(0);

  const items = useMemo(
    () => buildContextMenuItems(state.commands, menu.fullPath, menu.name, menu.kind),
    [state.commands, menu.fullPath, menu.name, menu.kind],
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
      });
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
      <div className="context-menu__overlay" onMouseDown={close} onContextMenu={(e) => e.preventDefault()}>
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
