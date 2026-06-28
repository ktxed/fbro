import {
  autoUpdate,
  flip,
  FloatingPortal,
  offset,
  shift,
  useFloating,
} from "@floating-ui/react";
import { useEffect, useLayoutEffect, useRef } from "react";
import { useAppState } from "../store";
import { resolveColorScheme } from "../theme/colorSchemes";

interface InputHistoryDropdownProps {
  anchorEl: HTMLElement | null;
  items: string[];
  activeIndex: number;
  onSelect: (item: string) => void;
  onClose: () => void;
}

export function InputHistoryDropdown({
  anchorEl,
  items,
  activeIndex,
  onSelect,
  onClose,
}: InputHistoryDropdownProps) {
  const state = useAppState();
  const popoverRef = useRef<HTMLDivElement>(null);
  const colorScheme = resolveColorScheme(state.settings.general.colorScheme);

  const { refs, floatingStyles } = useFloating({
    open: true,
    placement: "bottom-start",
    strategy: "fixed",
    whileElementsMounted: autoUpdate,
    middleware: [offset(2), flip({ padding: 8 }), shift({ padding: 8 })],
  });

  useLayoutEffect(() => {
    if (anchorEl) refs.setReference(anchorEl);
  }, [refs, anchorEl]);

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (
        popoverRef.current &&
        !popoverRef.current.contains(e.target as Node) &&
        anchorEl &&
        !anchorEl.contains(e.target as Node)
      ) {
        onClose();
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  });

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("keydown", handler, true);
    return () => document.removeEventListener("keydown", handler, true);
  });

  if (items.length === 0) return null;

  return (
    <FloatingPortal>
      <div
        data-color-scheme={colorScheme.id}
        ref={(node) => {
          refs.setFloating(node);
          (popoverRef as React.MutableRefObject<HTMLDivElement | null>).current = node;
        }}
        style={{ ...floatingStyles, colorScheme: colorScheme.mode }}
        className="input-history"
        onMouseDown={(e) => e.preventDefault()}
      >
        <ul className="input-history__list">
          {items.map((item, i) => (
            <li
              key={`${i}-${item}`}
              className={`input-history__item${i === activeIndex ? " input-history__item--active" : ""}`}
              onClick={() => onSelect(item)}
            >
              {item}
            </li>
          ))}
        </ul>
      </div>
    </FloatingPortal>
  );
}
