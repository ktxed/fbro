import {
  autoUpdate,
  flip,
  FloatingPortal,
  offset,
  shift,
  useFloating,
} from "@floating-ui/react";
import { useEffect, useLayoutEffect, useRef } from "react";
import { useAppDispatch } from "../store";
import type { BookmarkEntry } from "../ws/types";
import type { WsClient } from "../ws/client";

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
  const dispatch = useAppDispatch();
  const popoverRef = useRef<HTMLDivElement>(null);

  const close = () => dispatch({ type: "CLOSE_BOOKMARKS" });

  const { refs, floatingStyles } = useFloating({
    open: true,
    placement: "bottom-end",
    strategy: "fixed",
    whileElementsMounted: autoUpdate,
    middleware: [offset(4), flip({ padding: 8 }), shift({ padding: 8 })],
  });

  useLayoutEffect(() => {
    if (anchorEl) refs.setReference(anchorEl);
  }, [refs, anchorEl]);

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

  // Close on Escape
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.stopPropagation(); close(); }
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  });

  const lastName = currentPath.trim().replace(/\/$/, "").split("/").filter(Boolean).pop() ?? currentPath;
  const isBookmarked = bookmarks.some((b) => b.path === currentPath);

  const handleAdd = () => {
    client.sendIntent("AddBookmark", undefined, { name: lastName, path: currentPath });
  };

  const handleRemoveCurrent = () => {
    client.sendIntent("RemoveBookmark", undefined, { path: currentPath });
  };

  const handleNavigate = (path: string) => {
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
        ref={(node) => {
          refs.setFloating(node);
          (popoverRef as React.MutableRefObject<HTMLDivElement | null>).current = node;
        }}
        style={floatingStyles}
        className="bookmarks-popover"
      >
        <div className="bookmarks-popover__header">Bookmarks</div>

        <div className="bookmarks-popover__add-row">
          {isBookmarked ? (
            <button
              className="bookmarks-popover__add-btn bookmarks-popover__add-btn--added"
              onClick={handleRemoveCurrent}
              title="Remove current folder from bookmarks"
            >
              ✓ {lastName}
            </button>
          ) : (
            <button
              className="bookmarks-popover__add-btn"
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
            <ul className="bookmarks-popover__list">
              {bookmarks.map((bm) => (
                <li
                  key={bm.path}
                  className={`bookmarks-popover__item${bm.path === currentPath ? " bookmarks-popover__item--active" : ""}`}
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
              ))}
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
