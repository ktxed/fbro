import type { InteractiveCommand, KeyBinding, KeyStroke } from "../ws/types";

export type KeyboardLikeEvent = Pick<KeyboardEvent, "key" | "code" | "shiftKey" | "ctrlKey" | "altKey" | "metaKey">;
const normalizeKey = (key: string) => key.length === 1 ? key.toLowerCase() : key;
const labels: Record<string, string> = { " ": "Space", ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right" };

export function formatBinding(binding: KeyBinding, mac = typeof navigator !== "undefined" && navigator.platform.startsWith("Mac")) {
  return binding.strokes.map(stroke => {
    const parts: string[] = [];
    if (stroke.meta) parts.push(mac ? "Cmd" : "Meta");
    if (stroke.ctrl) parts.push("Ctrl");
    if (stroke.alt) parts.push("Alt");
    if (stroke.shift) parts.push("Shift");
    parts.push(labels[stroke.key] ?? (stroke.key.length === 1 ? stroke.key.toUpperCase() : stroke.key));
    return parts.join("+");
  }).join(", ");
}

export function bindingMatches(event: KeyboardLikeEvent, binding: KeyBinding, mode?: "list" | "grid") {
  if (binding.strokes.length !== 1) return false;
  if (binding.modes?.length && (!mode || !binding.modes.includes(mode))) return false;
  const stroke = binding.strokes[0];
  const key = normalizeKey(stroke.key);
  // Matching and capture share one key interpretation, including logical
  // history brackets reported by WebKit with a physical digit code.
  const eventKey = canonicalEventKey(event);
  const shiftSymbol = eventKey === key && key.length === 1 && key.trim().length > 0 && /[^a-z0-9]/i.test(key) && !stroke.shift && event.shiftKey;
  return eventKey === key
    && (shiftSymbol || !!stroke.shift === event.shiftKey)
    && !!stroke.ctrl === event.ctrlKey && !!stroke.alt === event.altKey && !!stroke.meta === event.metaKey;
}

function canonicalEventKey(event: KeyboardLikeEvent) {
  // WebKit can emit bracket characters with Digit8/Digit9 and no modifiers.
  // Preserve history navigation rather than interpreting those symbols as digits.
  if (event.key === "[" || event.key === "]") return event.key;
  if (/^Key[A-Z]$/.test(event.code)) return event.code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(event.code)) return event.code.slice(5);
  return normalizeKey(event.key);
}

/** Use the same physical letter/digit convention as dispatch, including Option and non-Latin layouts. */
export function captureStroke(event: KeyboardLikeEvent & { isComposing?: boolean }): KeyStroke | null {
  if (event.isComposing || ["Shift", "Control", "Alt", "Meta", "AltGraph", "Dead", "Process", "Unidentified", "CapsLock", "NumLock", "ScrollLock"].includes(event.key)) return null;
  const key = canonicalEventKey(event);
  if (!key || !(key.length === 1 || /^(F([1-9]|1[0-9]|2[0-4])|Arrow(Up|Down|Left|Right)|Enter|Escape|Tab|Backspace|Delete|Insert|Home|End|PageUp|PageDown)$/.test(key))) return null;
  return { key, shift: event.shiftKey, ctrl: event.ctrlKey, alt: event.altKey, meta: event.metaKey };
}

export function applyCommandsUpdate<T extends { commands: Record<string, InteractiveCommand>; commandRevision: number }>(state: T, revision: number, commands: InteractiveCommand[]): T {
  if (revision <= state.commandRevision) return state;
  return { ...state, commandRevision: revision, commands: Object.fromEntries(commands.map(command => [command.id, command])) };
}

/** Explicit stale-edit review follows identity, never a possibly reused index. */
export function reviewBindingEdit(original: KeyBinding[], index: number | null, current: KeyBinding[]) {
  if (index === null) return { bindings: current, index: null };
  const identity = (binding: KeyBinding) => JSON.stringify({
    strokes: binding.strokes.map(stroke => [normalizeKey(stroke.key), !!stroke.shift, !!stroke.ctrl, !!stroke.alt, !!stroke.meta]),
    modes: binding.modes?.length ? [...binding.modes].sort() : ["grid", "list"],
  });
  const before = original[index];
  if (!before) return null;
  const nextIndex = current.findIndex(binding => identity(binding) === identity(before));
  return nextIndex < 0 ? null : { bindings: current, index: nextIndex };
}

/** Fixed application zoom must share capture's key interpretation. */
export function zoomAction(event: KeyboardLikeEvent, mac = typeof navigator !== "undefined" && navigator.platform.startsWith("Mac")): -1 | 0 | 1 | null {
  if (!(mac ? event.metaKey : event.ctrlKey)) return null;
  const key = canonicalEventKey(event);
  if (key === "+" || key === "=") return 1;
  if (key === "-") return -1;
  if (key === "0") return 0;
  return null;
}
