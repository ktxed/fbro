// Screenshot capture for the user guide: settle the UI, then write
// site/src/assets/screens/<name>.webp (dark scheme only, see AGENTS.md).
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { repoRoot } from "../smoke/config.mjs";
import { sleep, waitFor } from "../smoke/wait.mjs";

export const screensDir = path.join(repoRoot, "site/src/assets/screens");

function loadSharp() {
  // sharp ships with the site's dependencies; reuse it instead of adding one here.
  const require = createRequire(path.join(repoRoot, "site/package.json"));
  try {
    return require("sharp");
  } catch {
    throw new Error("sharp not found: run `npm --prefix site install` first");
  }
}

export async function settle(browser, { pointerAt = { x: 1, y: 1 } } = {}) {
  // Park the pointer where the scene wants it kept during the capture: by
  // default the window's top-left corner, so no row shows hover.
  await browser.performActions([{ type: "pointer", id: "mouse", parameters: { pointerType: "mouse" },
    actions: [{ type: "pointerMove", duration: 0, origin: "viewport", x: pointerAt.x, y: pointerAt.y }] }]);
  await browser.releaseActions();
  // Queued toasts follow each other, so require a quiet second, not one empty frame.
  await waitFor(async () => {
    const toast = () => browser.execute(() => !!document.querySelector('[data-testid="transient-hint"]'));
    if (await toast()) return false;
    await sleep(1000);
    return !(await toast());
  }, { timeout: 40_000, label: "toasts gone" });
  await browser.execute(() => {
    document.querySelectorAll(".react-tooltip, [role='tooltip']").forEach((el) => el.remove());
    // The Docker backend lists its bind mounts as sources; a real machine
    // doesn't have them, so leave them out of the guide's pictures.
    document.querySelectorAll(".panel__volume-btn[title]").forEach((el) => {
      if (/^\/(etc|artifacts|state|usr\/local\/bin)(\/|$)/.test(el.getAttribute("title"))) el.style.display = "none";
    });
  });
  // Let thumbnails, transitions and layout finish.
  await waitFor(() => browser.execute(() =>
    Array.from(document.images).every((img) => img.complete)), { timeout: 15_000, label: "images loaded" });
  await sleep(600);
}

export async function shot(browser, name, options = {}) {
  await settle(browser, options);
  const png = Buffer.from(await browser.takeScreenshot(), "base64");
  fs.mkdirSync(screensDir, { recursive: true });
  const out = path.join(screensDir, `${name}.webp`);
  await loadSharp()(png).webp({ quality: 86 }).toFile(out);
  console.log(`[docs] ${path.relative(repoRoot, out)}`);
}
