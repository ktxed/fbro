import { waitFor } from "../wait.mjs";

/** Check both settled focus and containment, including delayed focus effects. */
export async function expectModalFocus(browser, selector, label) {
  await waitFor(() => browser.execute((selector) => {
    const control = document.querySelector(selector);
    const top = document.querySelector('[data-modal-state="top"]');
    return control !== null && control === document.activeElement && top?.contains(control);
  }, selector), { timeout: 10_000, label });
  const stable = await browser.execute(async (selector) => {
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return document.querySelector(selector) === document.activeElement;
  }, selector);
  if (!stable) throw new Error(`${label}: focus moved away from the default control`);
}

export async function expectCoveredModal(browser, modalId) {
  const covered = await browser.execute((modalId) => {
    const layer = document.querySelector(`[data-modal-id="${modalId}"]`);
    return layer?.getAttribute("data-modal-state") === "background"
      && layer.hasAttribute("inert") && layer.getAttribute("aria-hidden") === "true"
      && !layer.contains(document.activeElement);
  }, modalId);
  if (!covered) throw new Error(`${modalId}: covered modal remained interactive`);
}
