export const ZOOM_STEPS = [9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24];
export const DEFAULT_FONT_SIZE = 13;

export function applyZoom(fontSize: number) {
  const root = document.documentElement.style;
  const scale = fontSize / DEFAULT_FONT_SIZE;
  root.setProperty("--font-size",   `${fontSize}px`);
  root.setProperty("--row-height",  `${fontSize + 9}px`);
  root.setProperty("--col-size-w",  `${Math.round(70  * scale)}px`);
  root.setProperty("--col-date-w",  `${Math.round(130 * scale)}px`);
  root.setProperty("--col-perms-w", `${Math.round(76  * scale)}px`);
}

export function nextZoomSize(current: number, delta: 1 | -1): number {
  const idx = ZOOM_STEPS.indexOf(current);
  const nextIdx = idx === -1
    ? ZOOM_STEPS.indexOf(DEFAULT_FONT_SIZE)
    : Math.max(0, Math.min(ZOOM_STEPS.length - 1, idx + delta));
  return ZOOM_STEPS[nextIdx];
}
