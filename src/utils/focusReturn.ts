/** A temporary input owner remembers where to hand focus back on removal. */
export interface FocusReturnTarget {
  element: HTMLElement | null;
  panelId: string | null;
  fallback?: FocusReturnTarget;
}

// Keep the opener chain when a menu launches a dialog in the same React commit.
const owners = new WeakMap<HTMLElement, FocusReturnTarget>();
let pendingHandoff: { target: FocusReturnTarget } | null = null;

export function isAvailableFocusTarget(element: HTMLElement): boolean {
  return element.isConnected && !element.hidden && element.offsetParent !== null
    && !element.matches(":disabled") && !element.closest("[inert]");
}

export function captureFocusReturnTarget(panelId?: string): FocusReturnTarget {
  const active = document.activeElement instanceof HTMLElement && document.activeElement !== document.body
    ? document.activeElement : null;
  let fallback: FocusReturnTarget | undefined;
  for (let parent = active; parent; parent = parent.parentElement) {
    fallback = owners.get(parent);
    if (fallback) break;
  }
  // React may remove the old owner before the successor's layout effect runs.
  if (!active) fallback = pendingHandoff?.target;
  return {
    element: active,
    panelId: active?.closest<HTMLElement>("[data-panel-id]")?.dataset.panelId
      ?? panelId ?? fallback?.panelId
      ?? document.querySelector<HTMLElement>(".panel--focused")?.dataset.panelId ?? null,
    fallback,
  };
}

export function registerFocusReturnTarget(owner: HTMLElement, target: FocusReturnTarget) {
  owners.set(owner, target);
}

/** Give a successor dialog a real opener before dispatching its open action. */
export function handoffFocusToOpener(owner: HTMLElement | null) {
  if (!owner || document.querySelector("[data-modal-id]")) return;
  const active = document.activeElement;
  if (active !== document.body && !owner.contains(active)) return;
  const target = owners.get(owner);
  if (target) resolveFocusReturnTarget(target)?.focus({ preventScroll: true });
}

function panelListingTarget(panelId: string | null): HTMLElement | null {
  const original = panelId
    ? document.querySelector<HTMLElement>(`[data-panel-id="${CSS.escape(panelId)}"]`) : null;
  const panel = original?.classList.contains("panel--focused")
    ? original : document.querySelector<HTMLElement>(".panel--focused");
  if (!panel || !isAvailableFocusTarget(panel)) return null;
  return panel.querySelector<HTMLElement>(".panel__empty-actions button:not(:disabled)") ?? panel;
}

export function resolveFocusReturnTarget(target: FocusReturnTarget): HTMLElement | null {
  for (let candidate: FocusReturnTarget | undefined = target; candidate; candidate = candidate.fallback) {
    const element = candidate.element;
    if (element && isAvailableFocusTarget(element)) {
      return element;
    }
  }
  return panelListingTarget(target.panelId);
}

/** One handoff after removal, never a background focus-recovery loop. */
export function restoreFocusReturn(target: FocusReturnTarget, owner?: HTMLElement) {
  // StrictMode can replay cleanup without removing the owner.
  if (owner?.isConnected) return;
  if (document.querySelector("[data-modal-id], .context-menu, .bookmarks-popover")) return;
  const active = document.activeElement;
  // An outside click or a successor surface may already have taken focus.
  if (active instanceof HTMLElement && active !== document.body && isAvailableFocusTarget(active)) return;
  resolveFocusReturnTarget(target)?.focus({ preventScroll: true });
}

export function scheduleFocusHandoff(target: FocusReturnTarget, restore: () => void) {
  const pending = { target };
  pendingHandoff = pending;
  requestAnimationFrame(() => {
    restore();
    if (pendingHandoff === pending) pendingHandoff = null;
  });
}

export function scheduleFocusReturn(target: FocusReturnTarget, owner?: HTMLElement) {
  scheduleFocusHandoff(target, () => restoreFocusReturn(target, owner));
}

/** A focused action being removed must hand input back to its panel listing. */
export function returnPanelFocusOnRemoval(element: HTMLElement, panelId: string) {
  return () => {
    if (element.contains(document.activeElement)) {
      scheduleFocusReturn({ element: null, panelId }, element);
    }
  };
}
