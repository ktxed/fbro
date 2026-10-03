import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickEntry, goToPanelPath } from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

export async function verifyThumbnailQueue(browser, observer) {
  const requestId = `thumbnail-queue-${Date.now()}`;
  observer.sendIntent("QueryThumbnailQueue", undefined, { requestId });
  await waitFor(() => observer.messages.some((message) =>
    message.type === "ThumbnailQueueReady" && message.requestId === requestId
    && message.queue.running === 0 && message.queue.queued === 0
    && message.queue.maxConcurrent > 0 && message.queue.tasks.length === 0
  ), { timeout: 10_000, label: "backend thumbnail queue snapshot" });

  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await browser.keys("j");
  await expectModalFocus(browser, '[data-modal-id="jobs"] .modal__btn--primary', "jobs Close focus");
  await browser.keys("Tab");
  await expectModalFocus(browser, "#jobs-tab-jobs", "jobs tab focus");
  await browser.keys("ArrowRight");
  await expectModalFocus(browser, "#jobs-tab-thumbnails", "thumbnail tab keyboard focus");
  await waitFor(async () => (await (await browser.$('[data-testid="thumbnail-queue"]')).getText())
    .includes("No thumbnail work queued."), { timeout: 10_000, label: "thumbnail queue empty state" });
  // Polling must leave the user's keyboard focus on the selected tab.
  await browser.execute(() => new Promise((resolve) => setTimeout(resolve, 1200)));
  await expectModalFocus(browser, "#jobs-tab-thumbnails", "thumbnail refresh preserves focus");
  await browser.keys("Tab");
  await expectModalFocus(browser, '[data-modal-id="jobs"] .modal__btn--primary', "thumbnail view has one tab stop");
  await browser.keys("Escape");
  await (await browser.$('[data-modal-id="jobs"]')).waitForDisplayed({ timeout: 10_000, reverse: true });
  const restored = await browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left");
  if (!restored) throw new Error("Jobs modal did not restore its trigger focus");
  await verifyThumbnailViewport(browser, observer);
}

async function verifyThumbnailViewport(browser, observer) {
  const originalPath = observer.state.panels.left.path;
  const originalMode = observer.state.panels.left.viewMode;
  const isGrid = () => browser.execute(() => document.querySelector('[data-testid="panel-left"] .panel__list')?.classList.contains("panel__list--grid") === true);
  const fixture = backendPath("a", `thumbnail-viewport-${Date.now()}`);
  const expandedName = "image-00-expanded-card-selection-with-a-long-name.bmp";
  fs.mkdirSync(hostPath(fixture), { recursive: true });
  // One-pixel, uncompressed 24-bit BMP: no fixture-generation tool dependency.
  const bmp = Buffer.alloc(58);
  bmp.write("BM"); bmp.writeUInt32LE(58, 2); bmp.writeUInt32LE(54, 10);
  bmp.writeUInt32LE(40, 14); bmp.writeInt32LE(1, 18); bmp.writeInt32LE(1, 22);
  bmp.writeUInt16LE(1, 26); bmp.writeUInt16LE(24, 28); bmp.writeUInt32LE(4, 34);
  bmp[54] = 128; bmp[55] = 64; bmp[56] = 255;
  for (let index = 0; index < 60; index += 1) {
    fs.writeFileSync(hostPath(path.posix.join(fixture, index === 0 ? expandedName : `image-${String(index).padStart(2, "0")}.bmp`)), bmp);
  }
  try {
    await goToPanelPath(browser, observer, "left", fixture);
    if (!await isGrid()) {
      await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
      await browser.keys("g");
    }
    await waitFor(() => browser.execute((expandedName) => {
      const list = document.querySelector('[data-testid="panel-left"] .panel__list');
      const first = list?.querySelector(`[data-entry-name="${expandedName}"] .panel__thumb-img--loaded`);
      const last = list?.querySelector('[data-entry-name="image-59.bmp"] .panel__thumb-img');
      return !!first && !last;
    }, expandedName), { timeout: 10_000, label: "only visible grid cards fetch thumbnails" });

    await clickEntry(browser, "left", expandedName);
    const expandedSelector = '[data-testid="panel-left"] [data-testid="panel-expanded-card"]';
    await (await browser.$(expandedSelector)).waitForDisplayed({ timeout: 5000 });
    const unselectedFill = await browser.execute(selector => getComputedStyle(document.querySelector(selector)).backgroundImage, expandedSelector);
    await browser.keys(" ");
    await waitFor(() => browser.execute(selector => {
      const card = document.querySelector(selector);
      return card?.classList.contains("panel__card--selected") && getComputedStyle(card).backgroundImage !== "none";
    }, expandedSelector), { timeout: 5000, label: "expanded card reflects keyboard selection" });
    await browser.keys(" ");
    await waitFor(() => browser.execute((selector, originalFill) => {
      const card = document.querySelector(selector);
      return card && !card.classList.contains("panel__card--selected") && getComputedStyle(card).backgroundImage === originalFill;
    }, expandedSelector, unselectedFill), { timeout: 5000, label: "expanded card reflects deselection" });

    const targetPath = path.posix.join(fixture, "image-59.bmp");
    await browser.execute((targetPath, port, epoch) => {
      window.__thumbnailQueueSmoke = { done: false, ok: false };
      void fetch(`http://127.0.0.1:${port}/__thumb__/511/${encodeURIComponent(targetPath)}?panelId=left&panelEpoch=${epoch}`, { cache: "no-store" })
        .then(async response => { await response.arrayBuffer(); window.__thumbnailQueueSmoke = { done: true, ok: response.ok }; })
        .catch(() => { window.__thumbnailQueueSmoke = { done: true, ok: false }; });
    }, targetPath, observer.state.fileServerPort, observer.state.panels.left.panelEpoch);
    await waitFor(() => {
      const requestId = `viewport-queue-${Date.now()}`;
      observer.sendIntent("QueryThumbnailQueue", undefined, { requestId });
      return observer.messages.some(message => message.type === "ThumbnailQueueReady"
        && message.queue.backgroundWaitMs > 0
        && message.queue.tasks.some(task => task.path === targetPath && task.size === 511
          && task.status === "Queued" && task.priority === "background"));
    }, { timeout: 5000, label: "offscreen thumbnail held during cooldown" });
    await browser.execute(() => {
      const list = document.querySelector('[data-testid="panel-left"] .panel__list');
      list.scrollTop = list.scrollHeight;
    });
    await waitFor(() => browser.execute(() => window.__thumbnailQueueSmoke?.done && window.__thumbnailQueueSmoke?.ok), {
      timeout: 5000, label: "newly visible thumbnail bypasses background cooldown",
    });
    await waitFor(() => browser.execute(() => !!document.querySelector(
      '[data-testid="panel-left"] [data-entry-name="image-59.bmp"] .panel__thumb-img--loaded',
    )), { timeout: 5000, label: "thumbnail rendered after scrolling" });

    const oldEpoch = observer.state.panels.left.panelEpoch;
    const abandonedPath = path.posix.join(fixture, expandedName);
    await browser.execute((targetPath, port, epoch) => {
      window.__thumbnailQueueSmoke = { done: false };
      void fetch(`http://127.0.0.1:${port}/__thumb__/510/${encodeURIComponent(targetPath)}?panelId=left&panelEpoch=${epoch}`, { cache: "no-store" })
        .then(async response => { await response.arrayBuffer(); window.__thumbnailQueueSmoke = { done: true, status: response.status }; });
    }, abandonedPath, observer.state.fileServerPort, oldEpoch);
    await waitFor(async () => {
      const queue = await queryQueue(observer);
      return queue.tasks.some(task => task.path === abandonedPath && task.size === 510 && task.status === "Queued");
    }, { timeout: 5000, label: "thumbnail queued before navigation" });
    await goToPanelPath(browser, observer, "left", originalPath);
    await waitFor(async () => {
      const queue = await queryQueue(observer);
      return !queue.tasks.some(task => task.path.startsWith(`${fixture}/`));
    }, { timeout: 5000, label: "navigation clears abandoned thumbnail work" });
    await waitFor(() => browser.execute(() => window.__thumbnailQueueSmoke?.done && window.__thumbnailQueueSmoke?.status === 204), {
      timeout: 5000, label: "abandoned thumbnail request released without result",
    });
    const lateStatus = await browser.execute(async (targetPath, port, epoch) => {
      const response = await fetch(`http://127.0.0.1:${port}/__thumb__/509/${encodeURIComponent(targetPath)}?panelId=left&panelEpoch=${epoch}`, { cache: "no-store" });
      return response.status;
    }, abandonedPath, observer.state.fileServerPort, oldEpoch);
    if (lateStatus !== 204) throw new Error(`Stale thumbnail request returned ${lateStatus}`);
    const queue = await queryQueue(observer);
    if (queue.tasks.some(task => task.path.startsWith(`${fixture}/`))) throw new Error("Stale request refilled abandoned thumbnail queue");

  } finally {
    if ((await isGrid()) !== (originalMode === "grid")) {
      await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
      await browser.keys("g");
    }
    await goToPanelPath(browser, observer, "left", originalPath);
    // Retain the fixture with the other smoke artifacts. Deleting a just-watched
    // directory can race the watcher switch and disturb the next flow's path.
    await browser.execute(() => { delete window.__thumbnailQueueSmoke; });
  }
}

let queueQuerySequence = 0;
async function queryQueue(observer) {
  const requestId = `thumbnail-check-${++queueQuerySequence}`;
  observer.sendIntent("QueryThumbnailQueue", undefined, { requestId });
  await waitFor(() => observer.messages.some(message => message.type === "ThumbnailQueueReady" && message.requestId === requestId), {
    timeout: 5000, label: "fresh thumbnail queue snapshot",
  });
  return observer.messages.find(message => message.type === "ThumbnailQueueReady" && message.requestId === requestId).queue;
}
