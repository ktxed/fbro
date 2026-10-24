import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import type { PanelSlot } from "../ws/types";
import type { WsClient } from "../ws/client";

interface Drag {
  panelId: string;
  pointerId: number;
  element: HTMLElement;
  startX: number;
  startY: number;
  x: number;
  y: number;
  started: boolean;
}

interface DropMarker {
  panelId: string;
  beforePanelId: string | null;
  left: number;
}

/** Only gesture feedback lives here. The backend applies the actual reorder. */
export function useTabReorder(slot: PanelSlot, client: WsClient, blocked: boolean) {
  const tabOrder = JSON.stringify(slot.tabPanelIds);
  const listRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<Drag | null>(null);
  const markerRef = useRef<DropMarker | null>(null);
  const suppressClickRef = useRef(false);
  const cleanupRef = useRef<(() => void) | null>(null);
  const [marker, setMarker] = useState<DropMarker | null>(null);
  const [draggedPanelId, setDraggedPanelId] = useState<string | null>(null);

  const updateMarker = () => {
    const list = listRef.current;
    const drag = dragRef.current;
    let next: DropMarker | null = null;
    if (list && drag?.started) {
      const bounds = list.getBoundingClientRect();
      if (drag.x >= bounds.left && drag.x <= bounds.right && drag.y >= bounds.top && drag.y <= bounds.bottom) {
        const siblings = Array.from(list.querySelectorAll<HTMLElement>("[data-tab-panel-id]"))
          .filter((el) => el.dataset.tabPanelId !== drag.panelId);
        const before = siblings.find((el) => {
          const rect = el.getBoundingClientRect();
          return drag.x < rect.left + rect.width / 2;
        });
        const edge = before?.getBoundingClientRect().left ?? siblings.at(-1)?.getBoundingClientRect().right;
        if (edge !== undefined) next = {
          panelId: drag.panelId,
          beforePanelId: before?.dataset.tabPanelId ?? null,
          left: edge - bounds.left + list.scrollLeft,
        };
      }
    }
    const previous = markerRef.current;
    markerRef.current = next;
    if (next?.panelId !== previous?.panelId || next?.beforePanelId !== previous?.beforePanelId || next?.left !== previous?.left) {
      setMarker(next);
    }
  };

  const finish = () => {
    const drag = dragRef.current;
    dragRef.current = null;
    markerRef.current = null;
    cleanupRef.current?.();
    cleanupRef.current = null;
    if (drag?.element.hasPointerCapture(drag.pointerId)) drag.element.releasePointerCapture(drag.pointerId);
    setMarker(null);
    setDraggedPanelId(null);
  };

  // A concurrent tab lifecycle change or a newly opened modal/menu invalidates
  // the gesture. Cleanup also runs when switching tabs removes this bar.
  useEffect(() => {
    return () => {
      const drag = dragRef.current;
      dragRef.current = null;
      markerRef.current = null;
      cleanupRef.current?.();
      cleanupRef.current = null;
      if (drag?.element.hasPointerCapture(drag.pointerId)) drag.element.releasePointerCapture(drag.pointerId);
    };
  }, []);
  useEffect(() => { finish(); }, [tabOrder, blocked]);

  const onPointerDown = (event: ReactPointerEvent<HTMLElement>, panelId: string) => {
    suppressClickRef.current = false;
    if (blocked || event.pointerType !== "mouse" || event.button !== 0 || slot.tabPanelIds.length < 2
      || (event.target instanceof Element && event.target.closest("button"))) return;
    // Keep the exact existing focus owner, including a listing or PTY in the
    // other panel. Pointer capture needs no focus handoff or restoration.
    event.preventDefault();
    const element = event.currentTarget;
    element.setPointerCapture(event.pointerId);
    dragRef.current = {
      panelId, pointerId: event.pointerId, element,
      startX: event.clientX, startY: event.clientY,
      x: event.clientX, y: event.clientY, started: false,
    };
    const onKeyDown = (key: KeyboardEvent) => {
      if (key.defaultPrevented || blocked || key.key !== "Escape" || !dragRef.current?.started) return;
      key.preventDefault();
      key.stopPropagation();
      finish();
    };
    const onBlur = () => finish();
    // Escape belongs to the transient drag before the panel/PTY. ModalStack's
    // window capture handler retains higher priority. No window key listener.
    document.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("blur", onBlur);
    let frame = 0;
    let previousTime = 0;
    const scrollFrame = (time: number) => {
      const drag = dragRef.current;
      const list = listRef.current;
      if (drag?.started && list) {
        const bounds = list.getBoundingClientRect();
        if (drag.y >= bounds.top && drag.y <= bounds.bottom && drag.x >= bounds.left && drag.x <= bounds.right) {
          const direction = drag.x < bounds.left + 24 ? -1 : drag.x > bounds.right - 24 ? 1 : 0;
          const before = list.scrollLeft;
          list.scrollLeft += direction * Math.min(time - previousTime, 32) * 0.4;
          if (list.scrollLeft !== before) updateMarker();
        }
      }
      previousTime = time;
      frame = requestAnimationFrame(scrollFrame);
    };
    frame = requestAnimationFrame(scrollFrame);
    cleanupRef.current = () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("blur", onBlur);
    };
  };

  const onPointerMove = (event: ReactPointerEvent) => {
    const drag = dragRef.current;
    if (!drag || event.pointerId !== drag.pointerId) return;
    if (!(event.buttons & 1)) { finish(); return; }
    drag.x = event.clientX;
    drag.y = event.clientY;
    if (!drag.started && Math.hypot(drag.x - drag.startX, drag.y - drag.startY) >= 6) {
      drag.started = true;
      suppressClickRef.current = true;
      setDraggedPanelId(drag.panelId);
    }
    updateMarker();
  };

  const onPointerUp = (event: ReactPointerEvent) => {
    const drag = dragRef.current;
    if (!drag || event.pointerId !== drag.pointerId || event.button !== 0) return;
    drag.x = event.clientX;
    drag.y = event.clientY;
    updateMarker();
    const drop = markerRef.current;
    if (!blocked && drag.started && drop) {
      const index = slot.tabPanelIds.indexOf(drag.panelId);
      const next = slot.tabPanelIds[index + 1] ?? null;
      if (drop.beforePanelId !== next) client.sendIntent("ReorderPanelTab", undefined, {
        slotId: slot.slotId, panelId: drag.panelId, beforePanelId: drop.beforePanelId,
      });
    }
    finish();
  };

  return { listRef, marker, draggedPanelId, suppressClickRef, onPointerDown, onPointerMove, onPointerUp, cancel: finish };
}
