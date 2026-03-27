export const isMac = navigator.platform.startsWith("Mac") || navigator.userAgent.includes("Mac");

/** Returns true when the platform modifier key is held (Cmd on macOS, Ctrl elsewhere). */
export function modKey(e: KeyboardEvent | React.KeyboardEvent): boolean {
  return isMac ? e.metaKey : e.ctrlKey;
}
