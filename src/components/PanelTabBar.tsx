import { useEffect, useRef, useState } from "react";
import { IconChevronLeft, IconChevronRight, IconTerminal2, IconX } from "@tabler/icons-react";
import type { PanelSlot, PanelState } from "../ws/types";
import type { WsClient } from "../ws/client";

const MAX_TAB_CHARS = 18;

function tabTitle(panel: PanelState | undefined): string {
  if (!panel) return "…";
  if (panel.tabTitle) return panel.tabTitle;
  if (panel.tabKind === "terminal") {
    const base = basename(panel.path) || "~";
    return base;
  }
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
  client: WsClient;
}

export function PanelTabBar({ slot, panels, client }: Props) {
  const listRef = useRef<HTMLDivElement>(null);
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
    <div className="panel__tabs">
      <button
        className="panel__tabs-arrow"
        disabled={!canLeft}
        tabIndex={-1}
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
        onScroll={updateArrows}
        onDoubleClick={(e) => {
          if (e.target === e.currentTarget) newTabFromActive();
        }}
      >
        {slot.tabPanelIds.map((panelId) => {
          const panel = panels[panelId];
          const isActive = panelId === slot.activePanelId;
          const title = tabTitle(panel);
          const displayed =
            title.length > MAX_TAB_CHARS ? title.slice(0, MAX_TAB_CHARS - 1) + "…" : title;
          return (
            <div
              key={panelId}
              className={`panel__tab${isActive ? " panel__tab--active" : ""}`}
              title={panel?.path ?? title}
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
      </div>
      <button
        className="panel__tabs-arrow"
        disabled={!canRight}
        tabIndex={-1}
        onClick={() => scroll(1)}
      >
        <IconChevronRight size={14} />
      </button>
    </div>
  );
}
