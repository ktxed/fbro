import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

const MAX_ENTRIES = 20;

function storageKey(key: string) {
  return `fbro.history.${key}`;
}

function loadHistory(key: string): string[] {
  try {
    const raw = localStorage.getItem(storageKey(key));
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) return parsed.filter((v) => typeof v === "string");
    return [];
  } catch {
    return [];
  }
}

function saveHistory(key: string, entries: string[]) {
  localStorage.setItem(storageKey(key), JSON.stringify(entries));
}

export interface InputHistory {
  onKeyDown: (e: React.KeyboardEvent<HTMLElement>) => void;
  addToHistory: (value: string) => void;
  isOpen: boolean;
  items: string[];
  activeIndex: number;
  inputRef: React.RefObject<HTMLInputElement | HTMLTextAreaElement | null>;
  open: () => void;
  close: () => void;
  selectItem: (item: string) => void;
  setActiveIndex: (i: number | ((prev: number) => number)) => void;
}

export function useInputHistory(
  key: string,
  onChange: (v: string) => void,
): InputHistory {
  const [entries, setEntries] = useState<string[]>(() => loadHistory(key));
  const [isOpen, setIsOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement>(null);
  const altHeldRef = useRef(false);
  const blurredByUs = useRef(false);

  useEffect(() => {
    setEntries(loadHistory(key));
  }, [key]);

  const open = useCallback(() => {
    if (entries.length > 0) {
      setIsOpen(true);
      setActiveIndex(-1);
    }
  }, [entries.length]);

  const openRef = useRef(open);
  openRef.current = open;

  useEffect(() => {
    const captureDown = (e: KeyboardEvent) => {
      // Several prompt/viewer histories can coexist. Only the focused input
      // (or the input we temporarily blurred for AltGraph) owns this shortcut.
      if (!inputRef.current || (document.activeElement !== inputRef.current && !blurredByUs.current)) return;
      if (e.key === "Alt" || e.key === "AltGraph" || e.code === "AltLeft" || e.code === "AltRight") {
        altHeldRef.current = true;
        if (e.code === "AltRight" && e.key !== "Alt") {
          if (document.activeElement === inputRef.current) {
            (document.activeElement as HTMLElement).blur();
            blurredByUs.current = true;
          }
        }
      }
      if (altHeldRef.current && e.key === "ArrowDown") {
        e.preventDefault();
        e.stopImmediatePropagation();
        openRef.current();
        if (blurredByUs.current) {
          inputRef.current?.focus();
          blurredByUs.current = false;
        }
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.key === "Alt" || e.key === "AltGraph" || e.code === "AltLeft" || e.code === "AltRight") {
        altHeldRef.current = false;
        if (blurredByUs.current && inputRef.current) {
          inputRef.current.focus();
          blurredByUs.current = false;
        }
      }
    };
    const blur = () => { altHeldRef.current = false; blurredByUs.current = false; };
    window.addEventListener("keydown", captureDown, true);
    document.addEventListener("keyup", up);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", captureDown, true);
      document.removeEventListener("keyup", up);
      window.removeEventListener("blur", blur);
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    (async () => {
      const fn = await listen("alt-arrow-down", () => {
        if (cancelled) return;
        if (document.activeElement === inputRef.current) {
          openRef.current();
        }
      });
      if (!cancelled) unlistenFn = fn;
    })();
    return () => {
      cancelled = true;
      unlistenFn?.();
    };
  }, []);

  const close = useCallback(() => {
    setIsOpen(false);
    setActiveIndex(-1);
  }, []);

  const addToHistory = useCallback(
    (v: string) => {
      const trimmed = v.trim();
      if (!trimmed) return;
      const filtered = entries.filter((e) => e !== trimmed);
      const updated = [trimmed, ...filtered].slice(0, MAX_ENTRIES);
      setEntries(updated);
      saveHistory(key, updated);
    },
    [entries, key],
  );

  const selectItem = useCallback(
    (item: string) => {
      onChange(item);
      close();
    },
    [onChange, close],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLElement>) => {
      if (!isOpen) return;

      if (e.key === "ArrowDown") {
        e.preventDefault();
        e.stopPropagation();
        setActiveIndex((prev) => (prev + 1) % entries.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        e.stopPropagation();
        setActiveIndex((prev) => (prev <= 0 ? entries.length - 1 : prev - 1));
        return;
      }
      if (e.key === "Enter" && activeIndex >= 0) {
        e.preventDefault();
        e.stopPropagation();
        selectItem(entries[activeIndex]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close();
        inputRef.current?.focus();
        return;
      }
    },
    [isOpen, entries, activeIndex, selectItem, close],
  );

  return {
    onKeyDown,
    addToHistory,
    isOpen,
    items: entries,
    activeIndex,
    inputRef,
    open,
    close,
    selectItem,
    setActiveIndex,
  };
}
