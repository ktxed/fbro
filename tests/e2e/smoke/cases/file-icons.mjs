import fs from "node:fs";
import { backendPath, hostPath } from "../config.mjs";
import { goToPanelPath, pressWindowKey } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function verifyFileIcons(browser, observer) {
  const previous = observer.state.panels.left.path;
  const directory = backendPath("a", "icon-fixtures");
  const names = ["page.HTML", "photo.png", "photo.jpg", "data.json", "config.toml"];
  fs.mkdirSync(hostPath(directory), { recursive: true });
  for (const name of names) fs.writeFileSync(hostPath(`${directory}/${name}`), "icon fixture");
  try {
    await goToPanelPath(browser, observer, "left", directory);
    for (const name of names) {
      await observer.waitForEntry("left", name, "file");
      const expectedKey = `file-type-${Buffer.from(name.split(".").at(-1).toLowerCase()).toString("hex")}`;
      await waitFor(() => browser.execute((name, key) => {
        const row = Array.from(document.querySelectorAll('[data-testid="panel-left"] [data-testid="panel-entry"]'))
          .find(row => row.getAttribute("data-entry-name") === name);
        const image = row?.querySelector(".panel__col-icon img");
        return image && new URL(image.src).pathname.endsWith(`/__icon__/${key}`) && image.complete && image.naturalWidth > 0
          && getComputedStyle(image).display !== "none";
      }, name, expectedKey), { timeout: 10000, label: `loaded extension icon for ${name}` });
    }
    await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
    await pressWindowKey(browser, ",", { ctrl: true });
    await pressWindowKey(browser, ",", { meta: true });
    await browser.$('[data-modal-id="settings"]').waitForDisplayed();
    await browser.$('[data-testid="settings-tab-display"]').click();
    const supported = Boolean(observer.state.settings.general.iconCacheDir);
    const section = await browser.$('[data-testid="file-icon-cache-settings"]');
    if (await section.isExisting() !== supported) throw new Error("Icon cache control capability mismatch");
    if (supported) {
      const previousRevision = observer.state.settings.general.iconCacheRevision;
      await browser.execute(() => document.querySelector('[data-testid="clear-file-icon-cache"]')?.focus());
      await browser.keys("Enter");
      await waitFor(() => observer.state.settings.general.iconCacheRevision > previousRevision,
        { timeout: 10000, label: "keyboard clear invalidates native icons" });
      await waitFor(() => browser.execute(() => {
        const button = document.querySelector('[data-testid="clear-file-icon-cache"]');
        return button && !button.disabled && document.activeElement === button;
      }), { timeout: 10000, label: "clear completes and preserves settings focus" });
    }
    await browser.keys("Escape");
    await browser.$('[data-modal-id="settings"]').waitForDisplayed({ reverse: true });
  } finally {
    await goToPanelPath(browser, observer, "left", previous);
    fs.rmSync(hostPath(directory), { recursive: true, force: true });
  }
}
