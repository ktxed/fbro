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
