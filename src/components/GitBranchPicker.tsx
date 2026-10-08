import { autoUpdate, flip, FloatingFocusManager, FloatingPortal, offset, shift, size, useDismiss, useFloating, useInteractions } from "@floating-ui/react";
import { IconCheck, IconChevronDown } from "@tabler/icons-react";
import { forwardRef, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useAppState } from "../store";
import { resolveColorScheme } from "../theme/colorSchemes";
import { mergeRefs } from "../utils/mergeRefs";
import { rovingIndexForKey } from "./modalKeyboard";

interface GitBranchPickerProps {
  branches: string[];
  value: string;
  disabled: boolean;
  onSelect: (branch: string) => void;
  onInteract: () => void;
}

export const GitBranchPicker = forwardRef<HTMLButtonElement, GitBranchPickerProps>(function GitBranchPicker(
  { branches, value, disabled, onSelect, onInteract }, forwardedRef,
) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const selectionPendingRef = useRef(false);
  const listId = useId();
  const colorScheme = resolveColorScheme(useAppState().settings.general.colorScheme);
  const matches = useMemo(() => {
    const pattern = query.trim().toLowerCase();
    return branches.filter(branch => branch.toLowerCase().includes(pattern));
  }, [branches, query]);
  const visibleIndex = Math.min(activeIndex, Math.max(0, matches.length - 1));
  const isOpen = open && !disabled;
  const { refs, floatingStyles, context } = useFloating({
    open: isOpen,
    onOpenChange: setOpen,
    placement: "bottom-start",
    strategy: "fixed",
    whileElementsMounted: autoUpdate,
    middleware: [offset(4), flip({ padding: 8 }), shift({ padding: 8 }), size({
      padding: 8,
      apply({ availableHeight, availableWidth, elements }) {
        Object.assign(elements.floating.style, {
          maxHeight: `${Math.max(0, Math.min(360, availableHeight))}px`,
          maxWidth: `${Math.max(0, availableWidth)}px`,
        });
      },
    })],
  });
  const dismiss = useDismiss(context, { escapeKey: false });
  const { getFloatingProps } = useInteractions([dismiss]);
  const referenceRef = useMemo(() => mergeRefs(triggerRef, forwardedRef, refs.setReference), [forwardedRef, refs.setReference]);

  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);
  useEffect(() => {
    if (isOpen) optionRefs.current[visibleIndex]?.scrollIntoView({ block: "nearest" });
  }, [isOpen, visibleIndex, matches]);

  const openPicker = () => {
    if (disabled) return;
    onInteract();
    selectionPendingRef.current = false;
    setQuery("");
    setActiveIndex(Math.max(0, branches.indexOf(value)));
    setOpen(true);
  };
  const selectBranch = (branch: string) => {
    if (disabled || selectionPendingRef.current) return;
    selectionPendingRef.current = true;
    setOpen(false);
    if (branch !== value) onSelect(branch);
  };
  const onSearchKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented || event.nativeEvent.isComposing) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      return;
    }
    // Assistive technology can focus an option directly. Its native button
    // activation must select that option, rather than the input's highlight.
    if (event.target !== inputRef.current) {
      event.stopPropagation();
      return;
    }
    if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
    const next = rovingIndexForKey(event.key, visibleIndex, matches.length, "vertical");
    if (next !== null || ["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      event.stopPropagation();
      if (next !== null) setActiveIndex(next);
    } else if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      if (matches[visibleIndex]) selectBranch(matches[visibleIndex]);
    }
  };

  // Keep the popup inside its owning layer so ModalStack contains its focus,
  // but outside the Git dialog's overflow:hidden surface.
  const portalRoot = triggerRef.current?.closest<HTMLElement>("[data-modal-id]") ?? null;
  return (
    <>
      <button
        ref={referenceRef}
        type="button"
        className="git-branch-picker__trigger"
        data-testid="git-branch-picker"
        data-branch={value}
        aria-label={`Branch: ${value}`}
        aria-keyshortcuts="b"
        aria-haspopup="listbox"
        aria-expanded={isOpen}
        aria-controls={isOpen ? listId : undefined}
        // Remain focusable during checkout so popup dismissal can return focus
        // to its opener even while activation is disabled.
        aria-disabled={disabled}
        onFocus={onInteract}
        onClick={() => { if (isOpen) setOpen(false); else openPicker(); }}
        onKeyDown={event => {
          if (event.defaultPrevented) return;
          if (event.key === "Escape" && isOpen) {
            event.preventDefault();
            event.stopPropagation();
            setOpen(false);
            return;
          }
          if (["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) {
            event.preventDefault();
            event.stopPropagation();
            openPicker();
          }
        }}
      >
        <span>{value}</span><IconChevronDown size={14} aria-hidden />
      </button>
      {isOpen && portalRoot && (
        <FloatingPortal root={portalRoot} preserveTabOrder={false}>
          <FloatingFocusManager context={context} modal={false} guards={false} initialFocus={inputRef} returnFocus={triggerRef}>
            <div
              ref={refs.setFloating}
              id={`${listId}-popup`}
              role="group"
              aria-label="Choose branch"
              data-testid="git-branch-dropdown"
              data-color-scheme={colorScheme.id}
              className="git-branch-picker__popup"
              style={{ ...floatingStyles, colorScheme: colorScheme.mode }}
              {...getFloatingProps({
                onKeyDown: onSearchKeyDown,
                onClick: event => event.stopPropagation(),
                // ModalStack moves Tab focus in capture phase, before Floating
                // UI's portal guards. Dismiss when that shared cycle leaves us.
                onBlurCapture: event => {
                  if (event.relatedTarget instanceof Node
                    && !event.currentTarget.contains(event.relatedTarget)
                    && !triggerRef.current?.contains(event.relatedTarget)) setOpen(false);
                },
                onPointerDownCapture: event => {
                  if (!(event.target instanceof Element && event.target.closest("input"))) event.preventDefault();
                },
              })}
            >
              <input
                ref={inputRef}
                role="combobox"
                aria-label="Search branches"
                aria-autocomplete="list"
                aria-expanded="true"
                aria-controls={listId}
                aria-activedescendant={matches[visibleIndex] ? `${listId}-${visibleIndex}` : undefined}
                data-testid="git-branch-search"
                placeholder="Search branches…"
                value={query}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                onChange={event => { setQuery(event.target.value); setActiveIndex(0); }}
              />
              <div id={listId} role="listbox" aria-label="Branches" className="git-branch-picker__list">
                {matches.map((branch, index) => (
                  <button
                    ref={node => { optionRefs.current[index] = node; }}
                    key={branch}
                    id={`${listId}-${index}`}
                    type="button"
                    role="option"
                    aria-selected={branch === value}
                    tabIndex={-1}
                    className="git-branch-picker__option"
                    data-testid="git-branch-option"
                    data-active={index === visibleIndex}
                    title={branch === value ? `${branch} (current branch)` : branch}
                    onMouseDown={event => event.preventDefault()}
                    onClick={() => selectBranch(branch)}
                  >
                    <span>{branch}</span>
                    {branch === value && <IconCheck size={14} aria-hidden />}
                  </button>
                ))}
                {matches.length === 0 && <div className="git-branch-picker__empty" role="status">No matching branches.</div>}
              </div>
            </div>
          </FloatingFocusManager>
        </FloatingPortal>
      )}
    </>
  );
});
