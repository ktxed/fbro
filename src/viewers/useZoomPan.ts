import { useRef, useState, useCallback, useEffect, type RefObject } from "react";

export const WHEEL_FACTOR = 1.2;

export interface ZoomState { scale: number; tx: number; ty: number; }

interface Options {
  minScale: number;
  maxScale: number;
  /** Allow drag-to-pan at any scale; otherwise only when zoomed past 1×. */
  alwaysPan?: boolean;
}

/**
 * Cursor-anchored wheel zoom and drag-to-pan for a transformed element.
 * All coordinates are relative to the container midpoint, so the target must
 * be centred in the container when tx/ty are zero.
 */
export function useZoomPan(
  containerRef: RefObject<HTMLElement | null>,
  { minScale, maxScale, alwaysPan = false }: Options,
) {
  const [zoom, setZoom] = useState<ZoomState>({ scale: 1, tx: 0, ty: 0 });
  // Mirror into a ref so stable callbacks can read the latest value.
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  const [isDragging, setIsDragging] = useState(false);
  const hasDraggedRef = useRef(false);

  // mx/my: anchor point relative to container centre (positive = right/down).
  const zoomAt = useCallback((factor: number, mx = 0, my = 0) => {
    setZoom(prev => {
      const s = Math.min(Math.max(prev.scale * factor, minScale), maxScale);
      if (s === minScale && minScale === 1) return { scale: 1, tx: 0, ty: 0 };
      const r = s / prev.scale;
      // Keep the content point under the anchor fixed in screen space.
      return { scale: s, tx: mx * (1 - r) + prev.tx * r, ty: my * (1 - r) + prev.ty * r };
    });
  }, [minScale, maxScale]);

  // ── Wheel zoom (cursor-anchored) ─────────────────────────────────────────

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const handleWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const mx = e.clientX - rect.left - rect.width / 2;
      const my = e.clientY - rect.top - rect.height / 2;
      zoomAt(e.deltaY < 0 ? WHEEL_FACTOR : 1 / WHEEL_FACTOR, mx, my);
    };
    el.addEventListener("wheel", handleWheel, { passive: false });
    return () => el.removeEventListener("wheel", handleWheel);
  }, [containerRef, zoomAt]);

  // ── Drag to pan ──────────────────────────────────────────────────────────

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button !== 0 || (!alwaysPan && zoomRef.current.scale <= 1)) return;
    e.preventDefault();
    hasDraggedRef.current = false;
    setIsDragging(true);
    const startX = e.clientX;
    const startY = e.clientY;
    const { tx: startTx, ty: startTy } = zoomRef.current;
    const onMove = (ev: MouseEvent) => {
      const dx = ev.clientX - startX;
      const dy = ev.clientY - startY;
      if (Math.abs(dx) + Math.abs(dy) > 3) hasDraggedRef.current = true;
      setZoom(prev => ({ ...prev, tx: startTx + dx, ty: startTy + dy }));
    };
    const onUp = () => {
      setIsDragging(false);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }, [alwaysPan]);

  return { zoom, setZoom, zoomAt, isDragging, hasDraggedRef, onMouseDown };
}
