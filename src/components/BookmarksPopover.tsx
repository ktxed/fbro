import {
  autoUpdate,
  flip,
  FloatingPortal,
  offset,
  shift,
  useFloating,
} from "@floating-ui/react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useAppDispatch, useAppState } from "../store";
import { resolveColorScheme } from "../theme/colorSchemes";
import type { BookmarkEntry } from "../ws/types";
import type { WsClient } from "../ws/client";
import { rovingIndexForKey } from "./modalKeyboard";
import { useFocusReturn } from "../hooks/useFocusReturn";
import { handoffFocusToOpener } from "../utils/focusReturn";

interface BookmarksPopoverProps {
  anchorEl: HTMLElement | null;
  panelId: string;
  currentPath: string;
  bookmarks: BookmarkEntry[];
  client: WsClient;
}

export function BookmarksPopover({
  anchorEl,
  panelId,
  currentPath,
  bookmarks,
  client,
}: BookmarksPopoverProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const popoverRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useFocusReturn(panelId);
  const [selectedIndex, setSelectedIndex] = useState(() => {
    const currentIndex = bookmarks.findIndex((bookmark) => bookmark.path === currentPath);
    return currentIndex >= 0 ? currentIndex : 0;
  });
  const [listMaxHeight, setListMaxHeight] = useState<number>();
  const colorScheme = resolveColorScheme(state.settings.general.colorScheme);

  const close = () => dispatch({ type: "CLOSE_BOOKMARKS" });

  const { refs, floatingStyles } = useFloating({
    open: true,
    placement: "bottom-end",
    strategy: "fixed",
    whileElementsMounted: autoUpdate,
    middleware: [offset(4), flip({ padding: 8 }), shift({ padding: 8 })],
  });

  const setPopoverRef = useCallback((node: HTMLDivElement | null) => {
    refs.setFloating(node);
    popoverRef.current = node;
    const cleanup = returnFocusRef(node);
    if (node) (listRef.current ?? addButtonRef.current)?.focus({ preventScroll: true });
    return cleanup;
  }, [refs.setFloating, returnFocusRef]);

  useLayoutEffect(() => {
    if (anchorEl) refs.setReference(anchorEl);
  }, [refs, anchorEl]);

  useLayoutEffect(() => {
    const panel = anchorEl?.closest<HTMLElement>(".panel");
    if (!panel) return;

    const updateListMaxHeight = () => setListMaxHeight(panel.clientHeight / 2);
    updateListMaxHeight();
    const observer = new ResizeObserver(updateListMaxHeight);
    observer.observe(panel);
    return () => observer.disconnect();
  }, [anchorEl]);

  useLayoutEffect(() => {
    (listRef.current ?? addButtonRef.current)?.focus({ preventScroll: true });
  }, [bookmarks.length]);

  useEffect(() => {
    setSelectedIndex((current) => Math.min(current, Math.max(0, bookmarks.length - 1)));
  }, [bookmarks.length]);

  useEffect(() => {
    const selected = listRef.current?.querySelector<HTMLElement>("[data-selected='true']");
    selected?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  // Close on outside click
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (
        popoverRef.current &&
        !popoverRef.current.contains(e.target as Node) &&
        anchorEl &&
        !anchorEl.contains(e.target as Node)
      ) {
        close();
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  });

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;

      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        close();
        return;
      }

      const target = event.target instanceof Node ? event.target : null;
      if (event.key === "ArrowDown" && target === addButtonRef.current && bookmarks.length > 0) {
        event.preventDefault();
        event.stopPropagation();
        listRef.current?.focus({ preventScroll: true });
        return;
      }
      if (
        event.key === "ArrowUp" &&
        selectedIndex === 0 &&
        target &&
        listRef.current?.contains(target)
      ) {
        event.preventDefault();
        event.stopPropagation();
        addButtonRef.current?.focus({ preventScroll: true });
        return;
      }

      const nextIndex = rovingIndexForKey(event.key, selectedIndex, bookmarks.length, "vertical");
      if (nextIndex !== null) {
        event.preventDefault();
        event.stopPropagation();
        setSelectedIndex(nextIndex);
        listRef.current?.focus({ preventScroll: true });
        return;
      }

      const nativeButton = event.target instanceof Element
        ? event.target.closest(".bookmarks-popover button")
        : null;
      if (event.key === "Enter" && !nativeButton) {
        const selected = bookmarks[selectedIndex];
        if (!selected) return;
        event.preventDefault();
        event.stopPropagation();
        handleNavigate(selected.path);
      }
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  }, [bookmarks, selectedIndex]);

  const lastName = currentPath.trim().replace(/\/$/, "").split("/").filter(Boolean).pop() ?? currentPath;
  const isBookmarked = bookmarks.some((b) => b.path === currentPath);

  const handleAdd = () => {
    client.sendIntent("AddBookmark", undefined, { name: lastName, path: currentPath });
  };

  const handleRemoveCurrent = () => {
    client.sendIntent("RemoveBookmark", undefined, { path: currentPath });
  };

  const handleNavigate = (path: string) => {
    handoffFocusToOpener(popoverRef.current);
    client.sendIntent("NavigatePanelTo", undefined, { panelId, path });
    close();
  };

  const handleRemove = (path: string, e: React.MouseEvent) => {
    e.stopPropagation();
    client.sendIntent("RemoveBookmark", undefined, { path });
  };

  return (
    <FloatingPortal>
      <div
        data-color-scheme={colorScheme.id}
        ref={setPopoverRef}
        style={{ ...floatingStyles, colorScheme: colorScheme.mode }}
        className="bookmarks-popover"
        data-testid="bookmarks-popover"
        onKeyDown={(e) => e.stopPropagation()}
        onMouseDown={(e) => e.preventDefault()}
      >
        <div className="bookmarks-popover__header">Bookmarks</div>

        <div className="bookmarks-popover__add-row">
          {isBookmarked ? (
            <button
              ref={addButtonRef}
              className="bookmarks-popover__add-btn bookmarks-popover__add-btn--added"
              data-testid="bookmark-current-toggle"
              onClick={handleRemoveCurrent}
              title="Remove current folder from bookmarks"
            >
              ✓ {lastName}
            </button>
          ) : (
            <button
              ref={addButtonRef}
              className="bookmarks-popover__add-btn"
              data-testid="bookmark-current-toggle"
              onClick={handleAdd}
              title="Bookmark current folder"
            >
              + {lastName}
            </button>
          )}
        </div>

        {bookmarks.length > 0 && (
          <>
            <div className="bookmarks-popover__divider" />
            <ul
              ref={listRef}
              className="bookmarks-popover__list"
              role="listbox"
              aria-label="Bookmarks"
              aria-activedescendant={bookmarks[selectedIndex] ? `bookmark-option-${selectedIndex}` : undefined}
              tabIndex={0}
              style={listMaxHeight === undefined ? undefined : { maxHeight: `${listMaxHeight}px` }}
            >
              {bookmarks.map((bm, index) => {
                const selected = index === selectedIndex;
                return (
                  <li
                    key={bm.path}
                    id={`bookmark-option-${index}`}
                    role="option"
                    aria-selected={selected}
                    data-selected={selected ? "true" : "false"}
                    data-testid="bookmark-option"
                    className={`bookmarks-popover__item${bm.path === currentPath ? " bookmarks-popover__item--active" : ""}${selected ? " bookmarks-popover__item--selected" : ""}`}
                    onMouseEnter={() => setSelectedIndex(index)}
                    onClick={() => handleNavigate(bm.path)}
                  >
                    <span className="bookmarks-popover__item-name">{bm.name}</span>
                    <span className="bookmarks-popover__item-path">{bm.path}</span>
                    <button
                      className="bookmarks-popover__item-remove"
                      onClick={(e) => handleRemove(bm.path, e)}
                      title="Remove bookmark"
                    >
                      ×
                    </button>
                  </li>
                );
              })}
            </ul>
          </>
        )}

        {bookmarks.length === 0 && (
          <div className="bookmarks-popover__empty">No bookmarks yet</div>
        )}
      </div>
    </FloatingPortal>
  );
}
