const NATIVE_ACTIVATION_SELECTOR = [
  "button",
  "a[href]",
  "input",
  "select",
  "textarea",
  "summary",
  "[contenteditable='true']",
  "[role='button']",
  "[role='option']",
  "[role='tab']",
].join(",");

export function isNativeActivationTarget(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(NATIVE_ACTIVATION_SELECTOR) !== null;
}

export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target instanceof HTMLTextAreaElement) return true;
  if (target instanceof HTMLInputElement) {
    return !["button", "checkbox", "radio", "reset", "submit"].includes(target.type);
  }
  return target.isContentEditable;
}

/** Lowercase Latin letter a key event stands for, or null. Uses the typed
 *  character when it is a Latin letter and falls back to the physical key
 *  (`KeyX`) otherwise, so letter shortcuts still work on non-Latin layouts
 *  and with macOS Option. */
export function shortcutLetter(event: { key: string; code: string }): string | null {
  if (/^[a-z]$/i.test(event.key)) return event.key.toLowerCase();
  const physical = /^Key([A-Z])$/.exec(event.code);
  return physical ? physical[1].toLowerCase() : null;
}

/** Case-insensitive letter shortcut match; see `shortcutLetter`. */
export function isLetterKey(event: { key: string; code: string }, letter: string): boolean {
  return shortcutLetter(event) === letter;
}

export type RovingOrientation = "horizontal" | "vertical";

export function rovingIndexForKey(
  key: string,
  current: number,
  count: number,
  orientation: RovingOrientation,
): number | null {
  if (count <= 0) return null;
  if (key === "Home") return 0;
  if (key === "End") return count - 1;
  const previous = orientation === "vertical" ? key === "ArrowUp" : key === "ArrowLeft";
  const next = orientation === "vertical" ? key === "ArrowDown" : key === "ArrowRight";
  if (previous) return (current - 1 + count) % count;
  if (next) return (current + 1) % count;
  return null;
}
