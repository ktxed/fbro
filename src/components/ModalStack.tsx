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
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { mergeRefs } from "../utils/mergeRefs";

interface ModalEntry {
  token: string;
  semanticId: string;
  element: HTMLDivElement;
  restoreFocus: HTMLElement | null;
  initialFocusRef?: RefObject<HTMLElement | null>;
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
  "[contenteditable='true']",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

function focusableElements(root: HTMLElement): HTMLElement[] {
  const visible = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
    .filter((element) => isAvailableFocusTarget(element) && element.tabIndex >= 0);
  return visible.filter((element) => {
    if (!(element instanceof HTMLInputElement) || element.type !== "radio" || !element.name) return true;
    const group = visible.filter((candidate): candidate is HTMLInputElement => (
      candidate instanceof HTMLInputElement
      && candidate.type === "radio"
      && candidate.name === element.name
      && candidate.form === element.form
    ));
    const selected = group.find((candidate) => candidate.checked) ?? group[0];
    return element === selected;
  });
}

function isAvailableFocusTarget(element: HTMLElement): boolean {
  return !element.hidden && element.offsetParent !== null && !element.matches(":disabled");
}

function focusLayer(entry: Pick<ModalEntry, "element" | "initialFocusRef">) {
  const root = entry.element;
  const preferred = entry.initialFocusRef?.current;
  const marked = Array.from(root.querySelectorAll<HTMLElement>("[data-modal-initial-focus], [autofocus]"))
    .find(isAvailableFocusTarget);
  const target = preferred && root.contains(preferred) && isAvailableFocusTarget(preferred)
    ? preferred
    : marked
      ?? focusableElements(root)[0]
      ?? root;
  target.focus({ preventScroll: true });
  target.scrollIntoView({ block: "nearest", inline: "nearest" });
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
      if (restore?.isConnected && isAvailableFocusTarget(restore) && (!top || top.element.contains(restore))) {
        restore.focus({ preventScroll: true });
      } else if (top) {
        focusLayer(top);
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
        focusLayer(top);
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
      // A list may claim Tab as its own navigation key (see modal-stack.md §4).
      if (event.target instanceof Element && event.target.closest("[data-modal-owns-tab]")) return;

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
      next.scrollIntoView({ block: "nearest", inline: "nearest" });
    };

    const onFocusIn = (event: FocusEvent) => {
      const current = entriesRef.current;
      const top = current[current.length - 1];
      if (!top || top.element.contains(event.target as Node)) return;
      focusLayer(top);
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
  /** Move focus to the default control when the dialog changes workflow phase. */
  focusKey?: string;
}

export const ModalLayer = forwardRef<HTMLDivElement, ModalLayerProps>(function ModalLayer(
  { modalId, initialFocusRef, focusKey, className = "", children, style, ...props },
  forwardedRef,
) {
  const context = useContext(ModalStackContext);
  if (!context) throw new Error("ModalLayer must be used inside ModalStackProvider");
  const token = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const wasTopRef = useRef(false);
  const previousFocusKeyRef = useRef(focusKey);
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
      initialFocusRef,
    });
    return () => context.unregister(token);
  }, [context.register, context.unregister, initialFocusRef, modalId, token]);

  useLayoutEffect(() => {
    if (isTop && (!wasTopRef.current || previousFocusKeyRef.current !== focusKey) && rootRef.current) {
      focusLayer({ element: rootRef.current, initialFocusRef });
    }
    wasTopRef.current = isTop;
    previousFocusKeyRef.current = focusKey;
  }, [focusKey, initialFocusRef, isTop]);

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
