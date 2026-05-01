const ZOOM_STEPS = [9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24];
const DEFAULT_FONT_SIZE = 13;
const STORAGE_KEY = "fbro.fontSize";

function applyZoom(fontSize: number) {
  const root = document.documentElement.style;
  const scale = fontSize / DEFAULT_FONT_SIZE;
  root.setProperty("--font-size",   `${fontSize}px`);
  root.setProperty("--row-height",  `${fontSize + 9}px`);
  root.setProperty("--col-size-w",  `${Math.round(70  * scale)}px`);
  root.setProperty("--col-date-w",  `${Math.round(130 * scale)}px`);
  root.setProperty("--col-perms-w", `${Math.round(76  * scale)}px`);
}

export function loadZoom() {
  const stored = localStorage.getItem(STORAGE_KEY);
  const fontSize = stored ? parseInt(stored, 10) : DEFAULT_FONT_SIZE;
  applyZoom(Number.isFinite(fontSize) ? fontSize : DEFAULT_FONT_SIZE);
}

export function stepZoom(delta: 1 | -1) {
  const current = parseInt(
    getComputedStyle(document.documentElement).getPropertyValue("--font-size") || `${DEFAULT_FONT_SIZE}`,
    10,
  );
  const idx = ZOOM_STEPS.indexOf(current);
  const nextIdx = idx === -1
    ? ZOOM_STEPS.indexOf(DEFAULT_FONT_SIZE)
    : Math.max(0, Math.min(ZOOM_STEPS.length - 1, idx + delta));
  const next = ZOOM_STEPS[nextIdx];
  localStorage.setItem(STORAGE_KEY, String(next));
  applyZoom(next);
}

export function resetZoom() {
  localStorage.removeItem(STORAGE_KEY);
  applyZoom(DEFAULT_FONT_SIZE);
}
