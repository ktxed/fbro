import { forwardRef, useEffect, useRef, useState } from "react";
import type { KeyboardEventHandler, MouseEventHandler, PointerEventHandler, ReactNode, Ref } from "react";
import { useAppState } from "../store";
import type { WsClient } from "../ws/client";

type ModalSize = {
  width: number;
  height: number;
};

type ResizeDirection = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

interface ResizableModalProps {
  modalId: string;
  client: WsClient;
  className?: string;
  role?: string;
  ariaLabel: string;
  ariaModal?: boolean;
  tabIndex?: number;
  testId?: string;
  defaultSize: ModalSize;
  minSize?: ModalSize;
  maxViewportMargin?: number;
  onClick?: MouseEventHandler<HTMLDivElement>;
  onKeyDown?: KeyboardEventHandler<HTMLDivElement>;
  onPointerDownCapture?: PointerEventHandler<HTMLDivElement>;
  children: ReactNode;
}

const DEFAULT_MIN_SIZE: ModalSize = { width: 360, height: 240 };
const HANDLE_DIRECTIONS: ResizeDirection[] = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

function clampSize(size: ModalSize, minSize: ModalSize, margin: number): ModalSize {
  const maxWidth = Math.max(minSize.width, window.innerWidth - margin);
  const maxHeight = Math.max(minSize.height, window.innerHeight - margin);
  return {
    width: Math.min(Math.max(Math.round(size.width), minSize.width), maxWidth),
    height: Math.min(Math.max(Math.round(size.height), minSize.height), maxHeight),
  };
}

function mergeRefs<T>(...refs: Array<Ref<T> | undefined>) {
  return (value: T) => {
    for (const ref of refs) {
      if (!ref) continue;
      if (typeof ref === "function") {
        ref(value);
      } else {
        ref.current = value;
      }
    }
  };
}

export const ResizableModal = forwardRef<HTMLDivElement, ResizableModalProps>(function ResizableModal(
  {
    modalId,
    client,
    className = "",
    role = "dialog",
    ariaLabel,
    ariaModal,
    tabIndex,
    testId,
    defaultSize,
    minSize = DEFAULT_MIN_SIZE,
    maxViewportMargin = 48,
    onClick,
    onKeyDown,
    onPointerDownCapture,
    children,
  },
  forwardedRef,
) {
  const savedSize = useAppState().settings.modalSizes[modalId];
  const [size, setSize] = useState<ModalSize>(() => {
    const initial = savedSize ?? defaultSize;
    return clampSize(initial, minSize, maxViewportMargin);
  });
  const rootRef = useRef<HTMLDivElement | null>(null);
  const activeResize = useRef<{
    direction: ResizeDirection;
    pointerId: number;
    startX: number;
    startY: number;
    startSize: ModalSize;
  } | null>(null);

  const sizeFromPointer = (
    active: NonNullable<typeof activeResize.current>,
    clientX: number,
    clientY: number,
  ): ModalSize => {
    const dx = clientX - active.startX;
    const dy = clientY - active.startY;
    let width = active.startSize.width;
    let height = active.startSize.height;
    if (active.direction.includes("e")) width += dx;
    if (active.direction.includes("w")) width -= dx;
    if (active.direction.includes("s")) height += dy;
    if (active.direction.includes("n")) height -= dy;
    return clampSize({ width, height }, minSize, maxViewportMargin);
  };

  useEffect(() => {
    const next = clampSize(savedSize ?? defaultSize, minSize, maxViewportMargin);
    setSize(next);
  }, [defaultSize.height, defaultSize.width, maxViewportMargin, minSize.height, minSize.width, modalId, savedSize]);

  useEffect(() => {
    const onResize = () => {
      setSize((current) => clampSize(current, minSize, maxViewportMargin));
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      document.body.classList.remove("modal-resize-active");
    };
  }, [maxViewportMargin, minSize.height, minSize.width]);

  const startResize = (direction: ResizeDirection, e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    activeResize.current = {
      direction,
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      startSize: size,
    };
    document.body.classList.add("modal-resize-active");
  };

  const continueResize = (e: React.PointerEvent<HTMLDivElement>) => {
    const active = activeResize.current;
    if (!active || active.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    setSize(sizeFromPointer(active, e.clientX, e.clientY));
  };

  const endResize = (e: React.PointerEvent<HTMLDivElement>) => {
    const active = activeResize.current;
    if (!active || active.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    activeResize.current = null;
    document.body.classList.remove("modal-resize-active");
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // Pointer capture may already have been released by the browser.
    }
    const finalSize = sizeFromPointer(active, e.clientX, e.clientY);
    setSize(finalSize);
    client.sendIntent("SetModalSizePreference", undefined, {
      modalId,
      width: finalSize.width,
      height: finalSize.height,
    });
  };

  return (
    <div
      ref={mergeRefs(rootRef, forwardedRef)}
      className={`modal modal--resizable ${className}`.trim()}
      role={role}
      aria-modal={ariaModal}
      aria-label={ariaLabel}
      data-testid={testId}
      tabIndex={tabIndex}
      style={{ width: size.width, height: size.height }}
      onClick={(e) => {
        e.stopPropagation();
        onClick?.(e);
      }}
      onKeyDown={onKeyDown}
      onPointerDownCapture={onPointerDownCapture}
    >
      {children}
      {HANDLE_DIRECTIONS.map((direction) => (
        <div
          key={direction}
          className={`modal__resize-handle modal__resize-handle--${direction}`}
          aria-hidden="true"
          onPointerDown={(e) => startResize(direction, e)}
          onPointerMove={continueResize}
          onPointerUp={endResize}
          onPointerCancel={endResize}
        />
      ))}
    </div>
  );
});
