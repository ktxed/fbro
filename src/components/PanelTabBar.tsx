import { useEffect, useState } from "react";
import { IconChevronLeft, IconChevronRight, IconTerminal2, IconX } from "@tabler/icons-react";
import type { PanelSlot, PanelState, Source } from "../ws/types";
import type { WsClient } from "../ws/client";
import { isSearchSourceId } from "../utils/sourceId";
import { useTabReorder } from "../hooks/useTabReorder";

const MAX_TAB_CHARS = 18;

function sourceTabTitle(
  panel: PanelState,
  sources: Record<string, Source>,
): string | undefined {
  if (isSearchSourceId(panel.sourceId)) return panel.sourceId;
  if (panel.sourceId === "procs" || panel.sourceId === "journal") {
    return sources[panel.sourceId]?.name ?? panel.sourceId;
  }
  return undefined;
}

function tabTitle(
  panel: PanelState | undefined,
  sources: Record<string, Source>,
): string {
  if (!panel) return "…";
  if (panel.tabTitle) return panel.tabTitle;
  if (panel.tabKind === "terminal") {
    const base = basename(panel.path) || "~";
    return base;
  }
  const sourceTitle = sourceTabTitle(panel, sources);
  if (sourceTitle) return sourceTitle;
  return basename(panel.path) || "/";
}

function basename(path: string): string {
  if (!path) return "";
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  if (!normalized) return "/";
  const i = normalized.lastIndexOf("/");
  return i < 0 ? normalized : normalized.slice(i + 1);
}

interface Props {
  slot: PanelSlot;
  panels: Record<string, PanelState>;
  sources: Record<string, Source>;
  client: WsClient;
  blocked: boolean;
}

export function PanelTabBar({ slot, panels, sources, client, blocked }: Props) {
  const drag = useTabReorder(slot, client, blocked);
  const { listRef } = drag;
  const [canLeft, setCanLeft] = useState(false);
  const [canRight, setCanRight] = useState(false);

  const updateArrows = () => {
    const el = listRef.current;
    if (!el) return;
    setCanLeft(el.scrollLeft > 0);
    setCanRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 1);
  };

  useEffect(() => {
    const el = listRef.current;
    const id = requestAnimationFrame(updateArrows);
    if (!el) return () => cancelAnimationFrame(id);
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
    const observer = new ResizeObserver(updateArrows);
    observer.observe(el);
    return () => {
      cancelAnimationFrame(id);
      el.removeEventListener("wheel", onWheel);
      observer.disconnect();
    };
  }, [slot.tabPanelIds.length]);

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

  const closeTab = (panelId: string) => {
    if (slot.tabPanelIds.length <= 1) return;
    client.sendIntent("panel.tab.close", undefined, { panelId });
  };

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
          const tooltip = panel && sourceTabTitle(panel, sources)
            ? title
            : panel?.path ?? title;
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
              <span className="panel__tab-label">
                {panel?.tabKind === "terminal" && <IconTerminal2 size={16} />}
                {displayed}
              </span>
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
    </div>
  );
}
