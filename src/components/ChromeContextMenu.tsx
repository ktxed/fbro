import { autoUpdate, flip, FloatingPortal, offset, shift, useFloating } from "@floating-ui/react";
import { IconClipboardCopy, IconCopyPlus, IconFolderOpen, IconX, type Icon } from "@tabler/icons-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useAppDispatch, useAppState, type AppState, type PendingChromeMenu } from "../store";
import { useColorScheme } from "../theme/useColorScheme";
import { useFocusReturn } from "../hooks/useFocusReturn";
import { useCopyToClipboard } from "../hooks/useCopyToClipboard";
import { handoffFocusToOpener } from "../utils/focusReturn";
import type { WsClient } from "../ws/client";
import { requestCloseTab } from "./tabTitles";

type ChromeMenuAction = "duplicate" | "close" | "copy-path" | "navigate";

interface ChromeMenuItem {
  action: ChromeMenuAction;
  label: string;
}

const ACTION_ICONS: Record<ChromeMenuAction, Icon> = {
  duplicate: IconCopyPlus,
  close: IconX,
  "copy-path": IconClipboardCopy,
  navigate: IconFolderOpen,
};

/** A tab can be duplicated when the backend would clone it rather than open
 *  Home: a listing on a mounted local filesystem, outside an archive. */
function canDuplicateTab(state: AppState, panelId: string): boolean {
  const panel = state.panels[panelId];
  const source = panel ? state.sources[panel.sourceId] : undefined;
  return !!panel && !!source
    && panel.tabKind === "listing"
    && source.kind === "filesystem"
    && source.mounted
    && !source.shadow
    && !panel.shadowSource;
}

function menuItems(state: AppState, menu: PendingChromeMenu): ChromeMenuItem[] {
  if (menu.kind === "tab") {
    const slot = state.slots[menu.slotId];
    if (!slot?.tabPanelIds.includes(menu.tabPanelId)) return [];
    return [
      ...(canDuplicateTab(state, menu.tabPanelId) ? [{ action: "duplicate" as const, label: "Duplicate" }] : []),
      ...(slot.tabPanelIds.length > 1 ? [{ action: "close" as const, label: "Close" }] : []),
    ];
  }
  return [
    { action: "copy-path", label: "Copy path" },
    ...(menu.navigate ? [{ action: "navigate" as const, label: "Navigate to" }] : []),
  ];
}

/** Right-click menu for panel chrome — tab chips and breadcrumb segments.
 *  Mouse-opened only; focus stays with (and returns to) the panel listing. */
export function ChromeContextMenu({ client, menu }: { client: WsClient; menu: PendingChromeMenu }) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const copyToClipboard = useCopyToClipboard();
  const [highlighted, setHighlighted] = useState(0);
  const menuRef = useRef<HTMLDivElement>(null);
  const returnFocusRef = useFocusReturn(menu.panelId);
  const colorScheme = useColorScheme();
  const items = menuItems(state, menu);
  const reference = useMemo(() => ({
    getBoundingClientRect: () => DOMRect.fromRect({
      x: menu.anchorLeft,
      y: menu.anchorTop,
      width: menu.anchorRight - menu.anchorLeft,
      height: menu.anchorBottom - menu.anchorTop,
    }),
  }), [menu.anchorLeft, menu.anchorTop, menu.anchorRight, menu.anchorBottom]);
  const { refs, floatingStyles } = useFloating({
    open: true,
    placement: "bottom-start",
    strategy: "fixed",
    whileElementsMounted: autoUpdate,
    middleware: [offset(4), flip({ padding: 8 }), shift({ padding: 8 })],
  });
  useLayoutEffect(() => {
    refs.setReference(reference);
  }, [refs, reference]);
  const setMenuRef = useCallback((node: HTMLDivElement | null) => {
    refs.setFloating(node);
    menuRef.current = node;
    const cleanup = returnFocusRef(node);
    node?.focus({ preventScroll: true });
    return cleanup;
  }, [refs.setFloating, returnFocusRef]);
  const close = () => dispatch({ type: "CLOSE_CHROME_MENU" });

  // The tab or the folder the menu was opened on went away underneath it.
  const panelPath = state.panels[menu.panelId]?.path;
  const openedOnPath = useRef(panelPath);
  useEffect(() => {
    if (items.length === 0 || (menu.kind === "breadcrumb" && panelPath !== openedOnPath.current)) close();
  }, [items.length, menu.kind, panelPath]);

  const run = (item: ChromeMenuItem) => {
    handoffFocusToOpener(menuRef.current);
    close();
    if (menu.kind === "tab") {
      const slot = state.slots[menu.slotId];
      if (!slot) return;
      if (item.action === "duplicate") {
        client.sendIntent("panel.tab.new", undefined, {
          slotId: menu.slotId,
          source: "clone",
          panelId: menu.tabPanelId,
        });
      } else if (item.action === "close") {
        requestCloseTab(slot, state.panels, client, menu.tabPanelId);
      }
      return;
    }
    if (item.action === "copy-path") {
      void copyToClipboard(menu.copyPath, "path");
    } else if (item.action === "navigate" && menu.navigate) {
      client.sendIntent(menu.navigate.name, menu.navigate.scope, menu.navigate.args);
    }
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.defaultPrevented) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End") {
      event.preventDefault();
      event.stopPropagation();
      if (items.length === 0) return;
      setHighlighted((current) => event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      const item = items[highlighted];
      if (item) run(item);
    } else if (event.key === "Tab") {
      event.preventDefault();
      event.stopPropagation();
    }
  };

  return (
    <FloatingPortal>
      <div className="context-menu__overlay" data-color-scheme={colorScheme.id} data-color-scheme-base={colorScheme.base} style={{ colorScheme: colorScheme.mode }} onMouseDown={close} onContextMenu={(event) => event.preventDefault()}>
        <div
          className="context-menu"
          ref={setMenuRef}
          role="menu"
          tabIndex={-1}
          onKeyDown={onKeyDown}
          data-testid={`${menu.kind}-context-menu`}
          style={floatingStyles}
          onMouseDown={(event) => event.stopPropagation()}
        >
          {items.map((item, index) => {
            const ItemIcon = ACTION_ICONS[item.action];
            return (
            <button
              key={item.action}
              type="button"
              className={`context-menu__item${index === highlighted ? " context-menu__item--active" : ""}`}
              role="menuitem"
              data-testid={`${menu.kind}-menu-${item.action}`}
              onMouseEnter={() => setHighlighted(index)}
              onClick={() => run(item)}
            >
              <span className="context-menu__label">
                <ItemIcon className="context-menu__icon" size={15} aria-hidden="true" />
                {item.label}
              </span>
            </button>
            );
          })}
        </div>
      </div>
    </FloatingPortal>
  );
}
