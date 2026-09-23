import { useRef, useState, useCallback, useEffect, useMemo } from "react";
import { ViewerLoading } from "./ViewerLoading";
import { useZoomPan } from "./useZoomPan";

const SCALE_MAX = 16;
const SCALE_CLICK = 2;   // jump-to scale on click

interface Props { src: string; alt: string; }

/**
 * Image viewer with scroll-wheel zoom (cursor-anchored) and drag-to-pan.
 * Click toggles between fit and 2× zoom. z key also toggles.
 * All coordinates are centered on the container midpoint.
 */
export function ZoomableImage({ src, alt }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const { zoom, setZoom, isDragging, hasDraggedRef, onMouseDown } =
    useZoomPan(containerRef, { minScale: 1, maxScale: SCALE_MAX });
  const request = useMemo(() => ({ src }), [src]);
  const [imageState, setImageState] = useState<{ request: typeof request; status: "ready" | "error" } | null>(null);
  const currentRequest = useRef(request);
  currentRequest.current = request;
  const ready = imageState?.request === request && imageState.status === "ready";
  const failed = imageState?.request === request && imageState.status === "error";

  // Reset when the image changes (j/k navigation).
  useEffect(() => { setZoom({ scale: 1, tx: 0, ty: 0 }); }, [src, setZoom]);

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
  }, [hasDraggedRef, setZoom]);

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
  }, [setZoom]);

  const { scale, tx, ty } = zoom;
  const isZoomed = scale > 1;
  const cursor = isDragging ? "grabbing" : isZoomed ? "grab" : "zoom-in";

  return (
    <div
      ref={containerRef}
      className="viewer__image-container"
      style={{ overflow: "hidden", cursor }}
      onMouseDown={onMouseDown}
      onClick={handleClick}
      aria-busy={!ready && !failed}
    >
      {!ready && !failed && <ViewerLoading />}
      {failed && <div className="viewer__image-notice" role="alert">Unable to load image: {alt}</div>}
      <img
        key={src}
        src={src}
        alt={alt}
        draggable={false}
        onLoad={() => { if (currentRequest.current === request) setImageState({ request, status: "ready" }); }}
        onError={() => { if (currentRequest.current === request) setImageState({ request, status: "error" }); }}
        style={{
          visibility: ready ? "visible" : "hidden",
          position: failed ? "absolute" : undefined,
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
