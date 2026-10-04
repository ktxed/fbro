import fs from "node:fs";
import { backendPath, hostPath } from "../config.mjs";
import { pressWindowKey } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runHeicFlow(browser, observer) {
  const isMac = await browser.execute(() => navigator.platform.startsWith("Mac"));
  if (isMac) return;
  const mainUrl = await browser.getUrl();
  const previous = observer.state.settings.general.enableHeic2any;
  const file = backendPath("a", "invalid-preview.heic");
  const validFile = backendPath("a", "valid-preview.heic");
  fs.writeFileSync(hostPath(file), Buffer.from([0, 0, 0, 24, ...Buffer.from("ftypheic invalid") ]));
  try {
    observer.sendIntent("SetGeneralOption", undefined, { key: "enableHeic2any", value: false });
    await waitFor(() => observer.state.settings.general.enableHeic2any === false,
      { timeout: 5000, label: "HEIC preview starts disabled" });
    await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
    await pressWindowKey(browser, ",", { ctrl: true });
    await browser.$('[data-modal-id="settings"]').waitForDisplayed();
    const toggle = await browser.$('[data-testid="settings-enable-heic2any"]');
    await browser.execute(() => document.querySelector('[data-testid="settings-enable-heic2any"]')?.focus());
    await browser.keys(" ");
    await waitFor(() => observer.state.settings.general.enableHeic2any === true,
      { timeout: 5000, label: "keyboard enables HEIC preview through backend" });
    if (!(await toggle.isSelected())) throw new Error("HEIC checkbox did not reflect backend patch");
    await browser.keys("Escape");
    await browser.$('[data-modal-id="settings"]').waitForDisplayed({ reverse: true });
    const focusRestored = await browser.execute(() =>
      document.activeElement?.getAttribute("data-testid") === "panel-left");
    if (!focusRestored) throw new Error("Settings did not restore its opener focus");

    const params = new URLSearchParams({ viewer: "1", path: file, name: "invalid-preview.heic",
      source: observer.state.panels.left.sourceId, panel: "left", label: "main", local: "1",
      size: String(fs.statSync(hostPath(file)).size), fsport: String(observer.state.fileServerPort) });
    await browser.url(`${mainUrl.split("?")[0]}?${params}`);
    // Viewer clients get a lightweight startup patch, not a state snapshot.
    // The already-enabled preference must reach a newly opened F3 viewer.
    await browser.$('[data-testid="heic-preview"] [role="alert"]').waitForDisplayed({ timeout: 15000 });
    if (process.env.FBRO_HEIC_FIXTURE) {
      fs.copyFileSync(process.env.FBRO_HEIC_FIXTURE, hostPath(validFile));
      params.set("path", validFile);
      params.set("name", "valid-preview.heic");
      params.set("size", String(fs.statSync(hostPath(validFile)).size));
      await browser.url(`${mainUrl.split("?")[0]}?${params}`);
      await waitFor(() => browser.execute(() => {
        const image = document.querySelector(".viewer__image-container > img");
        return image?.complete && image.naturalWidth > 0;
      }), { timeout: 30000, label: "real HEIC fixture converts in newly opened viewer" });
    }
    observer.sendIntent("SetGeneralOption", undefined, { key: "enableHeic2any", value: false });
    await waitFor(() => browser.execute(() =>
      document.querySelector(".viewer__mode-btn--active")?.textContent.includes("Hex")),
    { timeout: 5000, label: "open viewer switches to hex when HEIC disabled" });
  } finally {
    observer.sendIntent("SetGeneralOption", undefined, { key: "enableHeic2any", value: previous });
    await browser.url(mainUrl);
    fs.unlinkSync(hostPath(file));
    if (fs.existsSync(hostPath(validFile))) fs.unlinkSync(hostPath(validFile));
  }
}
