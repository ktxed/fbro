// Inline zoom for mermaid diagrams in the Markdown preview. The preview DOM is
// injected imperatively (see ViewerApp), so these helpers build and drive
// plain DOM rather than React components.

export const MERMAID_ZOOM_STEPS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
const MIN_SCALE = MERMAID_ZOOM_STEPS[0];
const MAX_SCALE = MERMAID_ZOOM_STEPS[MERMAID_ZOOM_STEPS.length - 1];

export type MermaidAction = "out" | "in" | "fit" | "expand";

const ICONS: Record<MermaidAction, string> = {
  out: '<path d="M5 12h14"/>',
  in: '<path d="M12 5v14M5 12h14"/>',
  fit: '<path d="M4 8V4h4M16 4h4v4M20 16v4h-4M8 20H4v-4"/>',
  expand: '<path d="M14 4h6v6M10 20H4v-6M20 4l-7 7M4 20l7-7"/>',
};

const TITLES: Record<MermaidAction, string> = {
  out: "Zoom out (Ctrl+wheel)",
  in: "Zoom in (Ctrl+wheel)",
  fit: "Fit to width",
  expand: "Expand (wheel zoom, drag to pan)",
};

/** Natural (1×) size of a mermaid SVG, taken from its viewBox. */
export function mermaidNaturalSize(svg: SVGSVGElement): { width: number; height: number } | null {
  const vb = svg.viewBox?.baseVal;
  if (vb && vb.width > 0 && vb.height > 0) return { width: vb.width, height: vb.height };
  const rect = svg.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0 ? { width: rect.width, height: rect.height } : null;
}

function actionButton(action: MermaidAction): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "viewer__mermaid-btn";
  button.dataset.mermaidAction = action;
  button.title = TITLES[action];
  button.setAttribute("aria-label", TITLES[action]);
  // Icon-only: keeps button text out of copy/select-all of the preview.
  button.innerHTML = `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[action]}</svg>`;
  return button;
}

/** Wrap a sanitized mermaid SVG in the zoomable block with its toolbar. */
export function buildMermaidBlock(svg: SVGSVGElement): HTMLDivElement {
  const wrapper = document.createElement("div");
  wrapper.className = "viewer__mermaid";

  const toolbar = document.createElement("div");
  toolbar.className = "viewer__mermaid-toolbar";
  const label = document.createElement("span");
  label.className = "viewer__mermaid-scale";
  toolbar.append(actionButton("out"), label, actionButton("in"), actionButton("fit"), actionButton("expand"));

  const canvas = document.createElement("div");
  canvas.className = "viewer__mermaid-canvas";
  canvas.append(svg);

  wrapper.append(toolbar, canvas);
  // Remember mermaid's own sizing so "fit" restores it exactly.
  wrapper.dataset.fitStyle = svg.getAttribute("style") ?? "";
  setMermaidScale(wrapper, "fit");
  return wrapper;
}

export function mermaidSvgOf(wrapper: Element): SVGSVGElement | null {
  return wrapper.querySelector<SVGSVGElement>(":scope > .viewer__mermaid-canvas > svg");
}

/** Current effective scale relative to the diagram's natural size. */
function currentScale(wrapper: HTMLElement, svg: SVGSVGElement): number {
  const stored = Number(wrapper.dataset.scale);
  if (stored > 0) return stored;
  const natural = mermaidNaturalSize(svg);
  const width = svg.getBoundingClientRect().width;
  return natural && width > 0 ? width / natural.width : 1;
}

export function setMermaidScale(wrapper: HTMLElement, scale: number | "fit") {
  const svg = mermaidSvgOf(wrapper);
  const label = wrapper.querySelector<HTMLElement>(".viewer__mermaid-scale");
  if (!svg) return;
  const natural = mermaidNaturalSize(svg);
  if (scale === "fit" || !natural) {
    svg.setAttribute("style", wrapper.dataset.fitStyle ?? "");
    delete wrapper.dataset.scale;
    // Label via data attribute + CSS content so it never lands in copied text.
    label?.setAttribute("data-label", "Fit");
    return;
  }
  const s = Math.min(Math.max(scale, MIN_SCALE), MAX_SCALE);
  svg.style.maxWidth = "none";
  svg.style.width = `${natural.width * s}px`;
  svg.style.height = `${natural.height * s}px`;
  wrapper.dataset.scale = String(s);
  label?.setAttribute("data-label", `${Math.round(s * 100)}%`);
}

/** Step to the next preset zoom above (dir=1) or below (dir=-1) the current one. */
export function stepMermaidScale(wrapper: HTMLElement, dir: 1 | -1) {
  const svg = mermaidSvgOf(wrapper);
  if (!svg) return;
  const cur = currentScale(wrapper, svg);
  const eps = 0.01;
  const next = dir > 0
    ? MERMAID_ZOOM_STEPS.find(step => step > cur + eps) ?? MAX_SCALE
    : [...MERMAID_ZOOM_STEPS].reverse().find(step => step < cur - eps) ?? MIN_SCALE;
  setMermaidScale(wrapper, next);
}

/**
 * Ctrl/Cmd+wheel zoom anchored at the cursor: the diagram point under the
 * pointer stays put by adjusting the canvas (horizontal) and document
 * (vertical) scroll positions after resizing.
 */
export function wheelZoomMermaid(wrapper: HTMLElement, e: WheelEvent, scroller: HTMLElement | null) {
  const svg = mermaidSvgOf(wrapper);
  const canvas = svg?.parentElement;
  if (!svg || !canvas) return;
  const before = svg.getBoundingClientRect();
  if (before.width === 0 || before.height === 0) return;
  const px = (e.clientX - before.left) / before.width;
  const py = (e.clientY - before.top) / before.height;
  // Mouse notches (~100px) zoom by 1.2×; trackpad pinch deltas scale smoothly.
  const factor = Math.pow(1.2, Math.min(Math.max(-e.deltaY / 100, -1), 1));
  setMermaidScale(wrapper, currentScale(wrapper, svg) * factor);
  const after = svg.getBoundingClientRect();
  canvas.scrollLeft += after.left + px * after.width - e.clientX;
  if (scroller) scroller.scrollTop += after.top + py * after.height - e.clientY;
}
