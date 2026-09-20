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
