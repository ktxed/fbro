import { useLayoutEffect, useState, useSyncExternalStore } from "react";
import { IconAlertTriangle, IconChevronDown, IconChevronLeft, IconChevronRight, IconPencil, IconTerminal2, IconX } from "@tabler/icons-react";
import { editorStatus, editorStatusVersion, subscribeEditorStatus } from "../editor/sessions";
import type { PanelSlot, PanelState, Source } from "../ws/types";
import type { WsClient } from "../ws/client";
import { useAppDispatch, useAppState } from "../store";
import { formatPrimaryBinding } from "../commands/interactive";
import { useTabReorder } from "../hooks/useTabReorder";
import { requestCloseTab, sourceTabTitle, tabTitle } from "./tabTitles";

const MAX_TAB_CHARS = 18;
/** Width of the strip's edge fade, matching the mask in `styles.css`. A chip
 *  under it reads as clipped, so revealing a tab has to clear it. */
const STRIP_FADE_PX = 14;

interface Props {
  slot: PanelSlot;
  panels: Record<string, PanelState>;
  sources: Record<string, Source>;
  client: WsClient;
  blocked: boolean;
}

export function PanelTabBar({ slot, panels, sources, client, blocked }: Props) {
  const dispatch = useAppDispatch();
  const { commands, activeTabListPanelId } = useAppState();
  const drag = useTabReorder(slot, client, blocked);
  const { listRef } = drag;
  const [canLeft, setCanLeft] = useState(false);
  const [canRight, setCanRight] = useState(false);
  useSyncExternalStore(subscribeEditorStatus, editorStatusVersion);

  const updateArrows = () => {
    const el = listRef.current;
    if (!el) return;
    setCanLeft(el.scrollLeft > 0);
    setCanRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 1);
  };

  const count = slot.tabPanelIds.length;
  const tabListKey = formatPrimaryBinding(commands["panel.tab.list"]);
  const tabListOpen = activeTabListPanelId === slot.activePanelId;

  // Keep the active tab fully visible when it changes (tab shortcuts, a jump
  // from the tab list, new tab, ...), and whenever the strip itself is resized.
  // Clearing the edge fade is what makes the last chips of an overflowing
  // strip readable instead of half faded out. The observer matters because the
  // count button only appears after the first arrow update narrows the strip:
  // without a second pass the reveal would be measured against the wider strip
  // and leave the chip behind the new edge.
  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const reveal = () => {
      const tab = Array.from(el.querySelectorAll<HTMLElement>("[data-tab-panel-id]"))
        .find((t) => t.dataset.tabPanelId === slot.activePanelId);
      if (tab) {
        const box = el.getBoundingClientRect();
        const rect = tab.getBoundingClientRect();
        const left = box.left + STRIP_FADE_PX;
        const right = box.right - STRIP_FADE_PX;
        if (rect.left < left) el.scrollLeft -= left - rect.left;
        else if (rect.right > right) el.scrollLeft += rect.right - right;
      }
      updateArrows();
    };
    reveal();
    const onWheel = (event: WheelEvent) => {
      if (el.scrollWidth <= el.clientWidth) return;
      event.preventDefault();
      event.stopPropagation();
      const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY)
        ? event.deltaX
        : event.deltaY;
      if (delta !== 0) {
        el.scrollLeft += delta;
        updateArrows();
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    const observer = new ResizeObserver(reveal);
    observer.observe(el);
    return () => {
      el.removeEventListener("wheel", onWheel);
      observer.disconnect();
    };
  }, [slot.activePanelId, slot.tabPanelIds.length, tabListOpen]);

  const scroll = (dir: number) => {
    listRef.current?.scrollBy({ left: dir * 80, behavior: "smooth" });
  };

  const switchTo = (panelId: string) => {
    if (panelId === slot.activePanelId) return;
    client.sendIntent("panel.tab.switch", undefined, {
      slotId: slot.slotId,
      panelId,
    });
  };

  const closeTab = (panelId: string) => requestCloseTab(slot, panels, client, panelId);

  const newTabFromActive = () => {
    client.sendIntent("panel.tab.new", undefined, {
      slotId: slot.slotId,
      source: "clone",
    });
  };

  return (
    <div className="panel__tabs" data-testid={`panel-tabs-${slot.slotId}`}
      onPointerDownCapture={() => { drag.suppressClickRef.current = false; }}
      onClickCapture={(event) => {
        if (!drag.suppressClickRef.current) return;
        event.preventDefault();
        event.stopPropagation();
        drag.suppressClickRef.current = false;
      }}>
      <button
        className="panel__tabs-arrow"
        disabled={!canLeft}
        tabIndex={-1}
        title="Scroll tabs left"
        aria-label="Scroll tabs left"
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => scroll(-1)}
      >
        <IconChevronLeft size={14} />
      </button>
      <div
        className={[
          "panel__tabs-list",
          canLeft ? "panel__tabs-list--fade-left" : "",
          canRight ? "panel__tabs-list--fade-right" : "",
        ].filter(Boolean).join(" ")}
        ref={listRef}
        onPointerMove={drag.onPointerMove}
        onPointerUp={drag.onPointerUp}
        onPointerCancel={drag.cancel}
        onLostPointerCapture={drag.cancel}
        onDragStart={(event) => event.preventDefault()}
        onScroll={updateArrows}
        onDoubleClick={(e) => {
          if (e.target === e.currentTarget) newTabFromActive();
        }}
      >
        {slot.tabPanelIds.map((panelId) => {
          const panel = panels[panelId];
          const isActive = panelId === slot.activePanelId;
          const title = tabTitle(panel, sources);
          const displayed =
            title.length > MAX_TAB_CHARS ? title.slice(0, MAX_TAB_CHARS - 1) + "…" : title;
          const tooltip = panel?.editor
            ? panel.editor.filePath
            : panel && sourceTabTitle(panel, sources)
              ? title
              : panel?.path ?? title;
          const status = panel?.editor ? editorStatus(panel.editor.sessionId) : null;
          return (
            <div
              key={panelId}
              data-tab-panel-id={panelId}
              className={`panel__tab${isActive ? " panel__tab--active" : ""}${drag.draggedPanelId === panelId ? " panel__tab--dragging" : ""}`}
              title={tooltip}
              onPointerDown={(event) => drag.onPointerDown(event, panelId)}
              onClick={() => switchTo(panelId)}
              onAuxClick={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  closeTab(panelId);
                }
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                // Rename hook lands here in a future iteration.
              }}
            >
              {panel?.tabKind === "terminal" && (
                <IconTerminal2 className="panel__tab-icon" size={16} aria-hidden="true" />
              )}
              {panel?.tabKind === "editor" && (
                <IconPencil className="panel__tab-icon" size={15} aria-hidden="true" />
              )}
              <span className="panel__tab-label">
                {displayed}
              </span>
              {status?.problem ? (
                <IconAlertTriangle
                  className="panel__tab-warning"
                  size={12}
                  aria-label={status.problem === "error" ? "Save failed" : "Autosave paused"}
                  data-testid={`tab-editor-warning-${panelId}`}
                />
              ) : status?.pending ? (
                <span className="panel__tab-dot" aria-label="Saving" data-testid={`tab-editor-pending-${panelId}`} />
              ) : null}
              {slot.tabPanelIds.length > 1 && (
                <button
                  className="panel__tab-close"
                  tabIndex={-1}
                  title="Close tab"
                  aria-label="Close tab"
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(panelId);
                  }}
                >
                  <IconX size={10} />
                </button>
              )}
            </div>
          );
        })}
        {drag.marker && <span className="panel__tab-drop-marker" data-testid="tab-drop-marker"
          aria-hidden="true" style={{ left: drag.marker.left }} />}
      </div>
      <button
        className="panel__tabs-arrow"
        disabled={!canRight}
        tabIndex={-1}
        title="Scroll tabs right"
        aria-label="Scroll tabs right"
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => scroll(1)}
      >
        <IconChevronRight size={14} />
      </button>
      {canLeft || canRight ? (
        <button
          className="panel__tabs-count"
          type="button"
          disabled={blocked}
          tabIndex={-1}
          data-testid={`panel-tabs-count-${slot.slotId}`}
          data-tab-list-count="true"
          title={[`Show all ${count} tabs`, tabListKey].filter(Boolean).join(" · ")}
          aria-label={`Show all ${count} tabs`}
          aria-keyshortcuts={tabListKey || undefined}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => dispatch(tabListOpen
            ? { type: "CLOSE_TAB_LIST" }
            : { type: "OPEN_TAB_LIST", panelId: slot.activePanelId })}
        >
          <span className="panel__tabs-count-number">{count}</span>
          <IconChevronDown size={12} aria-hidden="true" />
        </button>
      ) : null}
    </div>
  );
}
