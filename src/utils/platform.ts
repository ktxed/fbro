export const isMac = navigator.platform.startsWith("Mac") || navigator.userAgent.includes("Mac");
export const isWindows = navigator.platform.startsWith("Win") || navigator.userAgent.includes("Windows");
export const isLinux = !isMac && !isWindows;

/** Returns true when the platform modifier key is held (Cmd on macOS, Ctrl elsewhere). */
export function modKey(e: KeyboardEvent | React.KeyboardEvent): boolean {
  return isMac ? e.metaKey : e.ctrlKey;
}
