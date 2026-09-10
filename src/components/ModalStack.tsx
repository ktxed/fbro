import {
  createContext,
  forwardRef,
  useCallback,
  useContext,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type HTMLAttributes,
  type ReactNode,
  type Ref,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";

interface ModalEntry {
  token: string;
  semanticId: string;
  element: HTMLDivElement;
  restoreFocus: HTMLElement | null;
}

interface ModalStackContextValue {
  entries: ModalEntry[];
  register: (entry: ModalEntry) => void;
  unregister: (token: string) => void;
}

const ModalStackContext = createContext<ModalStackContextValue | null>(null);

const FOCUSABLE_SELECTOR = [
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "a[href]",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

function focusableElements(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
    .filter((element) => !element.hidden && element.offsetParent !== null);
}

function focusLayer(root: HTMLElement, preferred?: HTMLElement | null) {
  const target = preferred && root.contains(preferred)
    ? preferred
    : root.querySelector<HTMLElement>("[data-modal-initial-focus], [autofocus]")
      ?? focusableElements(root)[0]
      ?? root;
  target.focus({ preventScroll: true });
}

export function ModalStackProvider({ children }: { children: ReactNode }) {
  const [entries, setEntries] = useState<ModalEntry[]>([]);
  const entriesRef = useRef(entries);
  entriesRef.current = entries;

  const register = useCallback((entry: ModalEntry) => {
    setEntries((current) => {
      if (current.some((candidate) => candidate.token === entry.token)) return current;
      const next = [...current, entry];
      entriesRef.current = next;
      return next;
    });
  }, []);

  const unregister = useCallback((token: string) => {
    let removed: ModalEntry | undefined;
    setEntries((current) => {
      removed = current.find((entry) => entry.token === token);
      const next = current
        .filter((entry) => entry.token !== token)
        .map((entry) => removed?.restoreFocus && entry.restoreFocus && removed.element.contains(entry.restoreFocus)
          ? { ...entry, restoreFocus: removed.restoreFocus }
          : entry);
      entriesRef.current = next;
      return next;
    });

    requestAnimationFrame(() => {
      const current = entriesRef.current;
      const top = current[current.length - 1];
      const restore = removed?.restoreFocus;
      if (restore?.isConnected && (!top || top.element.contains(restore))) {
        restore.focus({ preventScroll: true });
      } else if (top) {
        focusLayer(top.element);
      }
    });
  }, []);

  useLayoutEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const current = entriesRef.current;
      const top = current[current.length - 1];
      if (!top) return;

      if (!(event.target instanceof Node) || !top.element.contains(event.target)) {
        event.preventDefault();
        event.stopImmediatePropagation();
        focusLayer(top.element);
        top.element.dispatchEvent(new KeyboardEvent("keydown", {
          key: event.key,
          code: event.code,
          location: event.location,
          ctrlKey: event.ctrlKey,
          shiftKey: event.shiftKey,
          altKey: event.altKey,
          metaKey: event.metaKey,
          repeat: event.repeat,
          bubbles: true,
          cancelable: true,
          composed: true,
        }));
        return;
      }

      if (event.key !== "Tab") return;

      const controls = focusableElements(top.element);
      event.preventDefault();
      event.stopImmediatePropagation();
      if (controls.length === 0) {
        top.element.focus({ preventScroll: true });
        return;
      }

      const active = document.activeElement as HTMLElement | null;
      const index = active ? controls.indexOf(active) : -1;
      const next = event.shiftKey
        ? controls[(index <= 0 ? controls.length : index) - 1]
        : controls[(index + 1) % controls.length];
      next.focus({ preventScroll: true });
    };

    const onFocusIn = (event: FocusEvent) => {
      const current = entriesRef.current;
      const top = current[current.length - 1];
      if (!top || top.element.contains(event.target as Node)) return;
      focusLayer(top.element);
    };

    window.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("focusin", onFocusIn, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("focusin", onFocusIn, true);
    };
  }, []);

  const value = useMemo(() => ({ entries, register, unregister }), [entries, register, unregister]);
  return <ModalStackContext.Provider value={value}>{children}</ModalStackContext.Provider>;
}

export function useModalStack() {
  const context = useContext(ModalStackContext);
  if (!context) throw new Error("useModalStack must be used inside ModalStackProvider");
  return {
    depth: context.entries.length,
    hasActiveModal: context.entries.length > 0,
    topModalId: context.entries.at(-1)?.semanticId ?? null,
  };
}

interface ModalLayerProps extends HTMLAttributes<HTMLDivElement> {
  modalId: string;
  initialFocusRef?: RefObject<HTMLElement | null>;
}

function mergeRefs<T>(...refs: Array<Ref<T> | undefined>) {
  return (value: T | null) => {
    for (const ref of refs) {
      if (!ref) continue;
      if (typeof ref === "function") ref(value);
      else ref.current = value;
    }
  };
}

export const ModalLayer = forwardRef<HTMLDivElement, ModalLayerProps>(function ModalLayer(
  { modalId, initialFocusRef, className = "", children, style, ...props },
  forwardedRef,
) {
  const context = useContext(ModalStackContext);
  if (!context) throw new Error("ModalLayer must be used inside ModalStackProvider");
  const token = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const wasTopRef = useRef(false);
  const entryIndex = context.entries.findIndex((entry) => entry.token === token);
  const isTop = entryIndex >= 0 && entryIndex === context.entries.length - 1;

  useLayoutEffect(() => {
    const element = rootRef.current;
    if (!element) return;
    const active = document.activeElement;
    context.register({
      token,
      semanticId: modalId,
      element,
      restoreFocus: active instanceof HTMLElement ? active : null,
    });
    return () => context.unregister(token);
  }, [context.register, context.unregister, modalId, token]);

  useLayoutEffect(() => {
    if (isTop && !wasTopRef.current && rootRef.current) {
      focusLayer(rootRef.current, initialFocusRef?.current);
    }
    wasTopRef.current = isTop;
  }, [initialFocusRef, isTop]);

  const portalHost = document.querySelector<HTMLElement>(".app") ?? document.body;

  return createPortal(
    <div
      {...props}
      ref={mergeRefs(rootRef, forwardedRef)}
      className={`modal-overlay modal-layer ${className}`.trim()}
      data-modal-id={modalId}
      data-modal-depth={entryIndex < 0 ? undefined : entryIndex}
      data-modal-state={isTop ? "top" : "background"}
      aria-hidden={isTop ? undefined : true}
      inert={!isTop}
      tabIndex={props.tabIndex ?? -1}
      style={{ ...style, zIndex: 100 + Math.max(0, entryIndex) * 2 }}
    >
      {children}
    </div>,
    portalHost,
  );
});
