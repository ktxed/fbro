import { useRef, useState, useCallback, useEffect, useImperativeHandle, type Ref } from "react";
import { IconMinus, IconPlus, IconArrowsMinimize, IconX } from "@tabler/icons-react";
import { useZoomPan, WHEEL_FACTOR } from "./useZoomPan";
import { mermaidNaturalSize } from "./mermaidZoom";

const SCALE_MIN = 0.1;
const SCALE_MAX = 16;
const PAN_STEP = 60;  // px per arrow-key press

export interface MermaidOverlayHandle {
  zoomIn(): void;
  zoomOut(): void;
  fit(): void;
  /** Move the view by one step in the given direction (-1, 0, 1 per axis). */
  panBy(dirX: number, dirY: number): void;
}

interface Props {
  svg: SVGSVGElement;
  onClose: () => void;
  ref?: Ref<MermaidOverlayHandle>;
}

/**
 * Full-viewer view of one mermaid diagram with wheel zoom (cursor-anchored)
 * and drag-to-pan. Keys are routed by ViewerApp's single keydown handler
 * through the imperative handle.
 */
export function MermaidOverlay({ svg, onClose, ref }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const [natural, setNatural] = useState<{ width: number; height: number } | null>(null);
  const { zoom, setZoom, zoomAt, isDragging, onMouseDown } =
    useZoomPan(containerRef, { minScale: SCALE_MIN, maxScale: SCALE_MAX, alwaysPan: true });

  // Mount the diagram at its natural size; the stage transform does the zoom.
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    stage.replaceChildren(svg);
    const size = mermaidNaturalSize(svg);
    if (size) {
      svg.style.display = "block";
      svg.style.maxWidth = "none";
      svg.style.width = `${size.width}px`;
      svg.style.height = `${size.height}px`;
    }
    setNatural(size);
  }, [svg]);

  const fit = useCallback(() => {
    const el = containerRef.current;
    if (!el || !natural) return;
    const pad = 48;
    const scale = Math.min(
      (el.clientWidth - pad) / natural.width,
      (el.clientHeight - pad) / natural.height,
    );
    setZoom({ scale: Math.min(Math.max(scale, SCALE_MIN), SCALE_MAX), tx: 0, ty: 0 });
  }, [natural, setZoom]);

  useEffect(() => { fit(); }, [fit]);

  useImperativeHandle(ref, () => ({
    zoomIn: () => zoomAt(WHEEL_FACTOR),
    zoomOut: () => zoomAt(1 / WHEEL_FACTOR),
    fit,
    panBy: (dirX, dirY) => setZoom(prev => ({ ...prev, tx: prev.tx - dirX * PAN_STEP, ty: prev.ty - dirY * PAN_STEP })),
  }), [zoomAt, fit, setZoom]);

  const { scale, tx, ty } = zoom;

  return (
    <div className="viewer__mermaid-overlay" role="dialog" aria-label="Mermaid diagram">
      <div className="viewer__mermaid-overlay-toolbar">
        <button type="button" className="viewer__mode-btn" onClick={() => zoomAt(1 / WHEEL_FACTOR)} title="Zoom out (-)" aria-label="Zoom out">
          <IconMinus size={14} aria-hidden />
        </button>
        <span className="viewer__mermaid-overlay-scale">{Math.round(scale * 100)}%</span>
        <button type="button" className="viewer__mode-btn" onClick={() => zoomAt(WHEEL_FACTOR)} title="Zoom in (+)" aria-label="Zoom in">
          <IconPlus size={14} aria-hidden />
        </button>
        <button type="button" className="viewer__mode-btn" onClick={fit} title="Fit (0)" aria-label="Fit">
          <IconArrowsMinimize size={14} aria-hidden />
        </button>
        <button type="button" className="viewer__mode-btn" onClick={onClose} title="Back to document (Esc)" aria-label="Close">
          <IconX size={14} aria-hidden />
        </button>
      </div>
      <div
        ref={containerRef}
        className="viewer__mermaid-overlay-canvas"
        style={{ cursor: isDragging ? "grabbing" : "grab" }}
        onMouseDown={onMouseDown}
      >
        <div
          ref={stageRef}
          className="viewer__mermaid-overlay-stage"
          style={{
            visibility: natural ? "visible" : "hidden",
            transform: `translate(-50%, -50%) translate(${tx}px, ${ty}px) scale(${scale})`,
          }}
        />
      </div>
    </div>
  );
}
