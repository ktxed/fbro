import { autoUpdate, flip, FloatingPortal, offset, shift, useFloating } from "@floating-ui/react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useAppDispatch, useAppState, type EjectAction, type PendingSourceMenu } from "../store";
import { resolveColorScheme } from "../theme/colorSchemes";
import { useFocusReturn } from "../hooks/useFocusReturn";
import { handoffFocusToOpener } from "../utils/focusReturn";

export function SourceContextMenu({ menu }: { menu: PendingSourceMenu }) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const source = state.sources[menu.sourceId];
  const [highlighted, setHighlighted] = useState(0);
  const menuRef = useRef<HTMLDivElement>(null);
  const returnFocusRef = useFocusReturn();
  const colorScheme = resolveColorScheme(state.settings.general.colorScheme);
  const items: Array<{ action: EjectAction; label: string }> = source?.eject
    ? [
        { action: "unmount-volume", label: "Unmount volume…" },
        ...(source.eject.canEjectDrive ? [{ action: "eject-drive" as const, label: "Eject entire drive…" }] : []),
      ]
    : [];
  const reference = useMemo(() => ({
    getBoundingClientRect: () => DOMRect.fromRect({
      x: menu.anchorLeft,
      y: menu.anchorTop,
      width: menu.anchorRight - menu.anchorLeft,
      height: menu.anchorBottom - menu.anchorTop,
    }),
  }), [menu.anchorLeft, menu.anchorTop, menu.anchorRight, menu.anchorBottom]);
  const { refs, floatingStyles } = useFloating({
    open: true,
    placement: "bottom-start",
    strategy: "fixed",
    whileElementsMounted: autoUpdate,
    middleware: [offset(4), flip({ padding: 8 }), shift({ padding: 8 })],
  });
  useLayoutEffect(() => {
    refs.setReference(reference);
  }, [refs, reference]);
  const setMenuRef = useCallback((node: HTMLDivElement | null) => {
    refs.setFloating(node);
    menuRef.current = node;
    const cleanup = returnFocusRef(node);
    node?.focus({ preventScroll: true });
    return cleanup;
  }, [refs.setFloating, returnFocusRef]);
  const close = () => {
    dispatch({ type: "CLOSE_SOURCE_MENU" });
  };
  const run = (action: EjectAction) => {
    handoffFocusToOpener(menuRef.current);
    close();
    dispatch({ type: "OPEN_EJECT", eject: { sourceId: menu.sourceId, action, requestId: null, error: null } });
  };
  useEffect(() => {
    if (!source?.mounted || !source.eject) close();
  }, [source?.mounted, source?.eject]);
  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.defaultPrevented) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End") {
      event.preventDefault();
      event.stopPropagation();
      if (items.length === 0) return;
      setHighlighted((current) => event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      const item = items[highlighted];
      if (item) run(item.action);
    } else if (event.key === "Tab") {
      event.preventDefault();
      event.stopPropagation();
    }
  };

  return (
    <FloatingPortal>
      <div className="context-menu__overlay" data-color-scheme={colorScheme.id} style={{ colorScheme: colorScheme.mode }} onMouseDown={close} onContextMenu={(event) => event.preventDefault()}>
        <div
          className="context-menu"
          ref={setMenuRef}
          role="menu"
          tabIndex={-1}
          onKeyDown={onKeyDown}
          data-testid="source-context-menu"
          style={floatingStyles}
          onMouseDown={(event) => event.stopPropagation()}
        >
          {items.map((item, index) => (
            <button
              key={item.action}
              type="button"
              className={`context-menu__item${index === highlighted ? " context-menu__item--active" : ""}`}
              role="menuitem"
              data-testid={`source-${item.action}`}
              onMouseEnter={() => setHighlighted(index)}
              onClick={() => run(item.action)}
            >{item.label}</button>
          ))}
        </div>
      </div>
    </FloatingPortal>
  );
}
