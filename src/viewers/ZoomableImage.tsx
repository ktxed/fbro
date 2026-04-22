import { useRef, useState, useCallback, useEffect } from "react";

const SCALE_MAX = 16;
const SCALE_CLICK = 2;   // jump-to scale on click
const WHEEL_FACTOR = 1.2;

interface Props { src: string; alt: string; }

/**
 * Image viewer with scroll-wheel zoom (cursor-anchored) and drag-to-pan.
 * Click toggles between fit and 2× zoom. z key also toggles.
 * All coordinates are centered on the container midpoint.
 */
export function ZoomableImage({ src, alt }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [zoom, setZoom] = useState({ scale: 1, tx: 0, ty: 0 });
  // Mirror into a ref so stable callbacks can read the latest value.
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  const [isDragging, setIsDragging] = useState(false);
  const hasDraggedRef = useRef(false);

  // Reset when the image changes (j/k navigation).
  useEffect(() => { setZoom({ scale: 1, tx: 0, ty: 0 }); }, [src]);

  // ── Wheel zoom (cursor-anchored) ─────────────────────────────────────────

  const handleWheel = useCallback((e: WheelEvent) => {
    e.preventDefault();
    const el = containerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    // mx/my: mouse position relative to container centre (positive = right/down)
    const mx = e.clientX - rect.left - rect.width / 2;
    const my = e.clientY - rect.top - rect.height / 2;
    const factor = e.deltaY < 0 ? WHEEL_FACTOR : 1 / WHEEL_FACTOR;
    setZoom(prev => {
      const s = Math.min(Math.max(prev.scale * factor, 1), SCALE_MAX);
      if (s === 1) return { scale: 1, tx: 0, ty: 0 };
      const r = s / prev.scale;
      // Keep the image point under the cursor fixed in screen space.
      return { scale: s, tx: mx * (1 - r) + prev.tx * r, ty: my * (1 - r) + prev.ty * r };
    });
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    el.addEventListener("wheel", handleWheel, { passive: false });
    return () => el.removeEventListener("wheel", handleWheel);
  }, [handleWheel]);

  // ── Drag to pan ──────────────────────────────────────────────────────────

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    if (e.button !== 0 || zoomRef.current.scale <= 1) return;
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
  }, []);

  // ── Click to toggle fit / 2× ─────────────────────────────────────────────

  const handleClick = useCallback((e: React.MouseEvent) => {
    if (hasDraggedRef.current) { hasDraggedRef.current = false; return; }
    const el = containerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const mx = e.clientX - rect.left - rect.width / 2;
    const my = e.clientY - rect.top - rect.height / 2;
    setZoom(prev => {
      if (prev.scale > 1) return { scale: 1, tx: 0, ty: 0 };
      const s = SCALE_CLICK;
      // Centre the zoom on the clicked point.
      return { scale: s, tx: mx * (1 - s), ty: my * (1 - s) };
    });
  }, []);

  // ── z key: toggle fit / 2× ───────────────────────────────────────────────

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key !== "z" && e.key !== "Z") return;
      setZoom(prev =>
        prev.scale > 1
          ? { scale: 1, tx: 0, ty: 0 }
          : { scale: SCALE_CLICK, tx: 0, ty: 0 }
      );
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const { scale, tx, ty } = zoom;
  const isZoomed = scale > 1;
  const cursor = isDragging ? "grabbing" : isZoomed ? "grab" : "zoom-in";

  return (
    <div
      ref={containerRef}
      className="viewer__image-container"
      style={{ overflow: "hidden", cursor }}
      onMouseDown={handleMouseDown}
      onClick={handleClick}
    >
      <img
        src={src}
        alt={alt}
        draggable={false}
        style={{
          maxWidth: "100%",
          maxHeight: "100%",
          objectFit: "contain",
          transformOrigin: "center center",
          transform: isZoomed ? `translate(${tx}px, ${ty}px) scale(${scale})` : "none",
          userSelect: "none",
          cursor: "inherit",
          display: "block",
        }}
      />
    </div>
  );
}
