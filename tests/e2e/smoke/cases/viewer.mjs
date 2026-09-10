import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import { performance } from "node:perf_hooks";
import { execFileSync } from "node:child_process";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import { waitFor } from "../wait.mjs";

// Uses the real viewer component, WS navigation, prefetch queue and file server.
// A forwarding server can hold one image response to make loading/error tests deterministic.
export async function runViewerFlow(browser, observer) {
  const mainUrl = await browser.getUrl();
  const originalPath = observer.state.panels.left.path;
  const directory = backendPath("a", "viewer-images");
  fs.mkdirSync(hostPath(directory), { recursive: true });
  const encoded = await browser.execute(() => {
    const canvas = document.createElement("canvas");
    canvas.width = 3840;
    canvas.height = 5120;
    const ctx = canvas.getContext("2d");
    const pixels = ctx.createImageData(canvas.width, canvas.height);
    let seed = 17;
    for (let i = 0; i < pixels.data.length; i += 4) {
      seed = (Math.imul(seed, 1664525) + 1013904223) | 0;
      const shade = (seed >>> 24) & 255;
      pixels.data[i] = shade;
      pixels.data[i + 1] = shade;
      pixels.data[i + 2] = shade;
      pixels.data[i + 3] = 255;
    }
    ctx.putImageData(pixels, 0, 0);
    return canvas.toDataURL("image/jpeg", 0.10).split(",")[1];
  });
  const jpeg = Buffer.from(encoded, "base64");
  const names = Array.from({ length: 32 }, (_, i) => `${String(i).padStart(2, "0")}.jpg`);
  for (const name of names) fs.writeFileSync(hostPath(`${directory}/${name}`), jpeg);
  fs.writeFileSync(hostPath(`${directory}/32-broken.jpg`), "not an image");

  let hold = false;
  const held = new Set();
  const proxy = http.createServer(async (req, res) => {
    res.setHeader("Access-Control-Allow-Origin", "*");
    res.setHeader("Cache-Control", "no-store");
    if (hold) await new Promise(resolve => { held.add(resolve); res.once("close", resolve); });
    if (res.destroyed) return;
    try {
      const upstream = await fetch(`http://127.0.0.1:${observer.state.fileServerPort}${req.url}`);
      res.statusCode = upstream.status;
      res.setHeader("Content-Type", upstream.headers.get("content-type") ?? "image/jpeg");
      res.end(Buffer.from(await upstream.arrayBuffer()));
    } catch (error) { res.statusCode = 500; res.end(String(error)); }
  });
  await new Promise(resolve => proxy.listen(0, "127.0.0.1", resolve));
  const release = () => { hold = false; for (const resolve of held) resolve(); held.clear(); };
  const measurements = { width: 3840, height: 5120, compressedBytes: jpeg.length,
    forwardMs: [], reverseMs: [], memory: [] };
  const sampleMemory = phase => {
    const processes = execFileSync("ps", ["-eo", "pid=,ppid=,rss=,comm="], { encoding: "utf8" })
      .trim().split("\n").map(line => {
        const [pid, parent, rss, name] = line.trim().split(/\s+/);
        return { pid: Number(pid), parent: Number(parent), rssKiB: Number(rss), name };
      });
    const descendants = new Set([process.pid]);
    for (let i = 0; i < processes.length; i++) {
      for (const p of processes) if (descendants.has(p.parent)) descendants.add(p.pid);
    }
    const frontend = processes.filter(p => descendants.has(p.pid) && /^(fbro|WebKit)/.test(p.name));
    measurements.memory.push({ phase, frontend });
  };
  const ready = () => browser.execute(() => {
    const image = document.querySelector(".viewer__image-container > img");
    return !!image && image.complete && image.naturalWidth > 0
      && !document.querySelector(".viewer__loading-overlay");
  });
  const waitReady = label => waitFor(ready, { timeout: 15_000, interval: 25, label });
  const navigate = async (key, filename) => {
    const start = performance.now();
    await browser.keys(key);
    await waitFor(() => browser.execute(expected => document.querySelector(".viewer__filename")?.textContent === expected, filename),
      { timeout: 10_000, interval: 25, label: `viewer filename ${filename}` });
    await waitReady(`viewer image ${filename}`);
    return performance.now() - start;
  };
  try {
    // The backend must reject marked warmups independently of frontend flags.
    // /proc is a known virtual mount in the Linux smoke environment.
    for (const route of [
      "/__thumb__/128/%2Fproc%2Fself%2Fmountinfo?prefetch=1",
      "/__remote_thumb__/missing-source/128/0/0/image.jpg?prefetch=1",
    ]) {
      const response = await fetch(`http://127.0.0.1:${observer.state.fileServerPort}${route}`);
      if (response.status !== 204 || response.headers.get("cache-control") !== "no-store") {
        throw new Error(`Speculative request was not rejected without caching: ${route} (${response.status})`);
      }
    }
    observer.navigatePanelToPath("left", directory);
    await observer.waitForPanelPath("left", directory);
    await observer.waitForEntry("left", names[0]);
    const source = observer.state.sources[observer.state.panels.left.sourceId];
    if (!source?.storageClass || source.prefetchEligible !== (source.storageClass === "internal-ssd")) {
      throw new Error(`Missing or inconsistent source storage policy: ${JSON.stringify(source)}`);
    }
    measurements.storageClass = source.storageClass;
    measurements.prefetchEligible = source.prefetchEligible;
    observer.sendIntent("SetSort", "left", { by: "name", order: "asc" });
    await observer.waitForSort("left", "name", "asc");
    const index = observer.state.panels.left.entries.findIndex(e => e.name === names[0]);
    observer.sendIntent("SetCursor", "left", { index });
    observer.sendIntent("SetEmbeddedViewerActive", undefined, { active: true });
    const params = new URLSearchParams({ viewer: "1", path: `${directory}/${names[0]}`,
      source: observer.state.panels.left.sourceId, name: names[0], label: "main", panel: "left",
      local: "1", size: String(jpeg.length), fsport: String(proxy.address().port), imgcap: String(10 * 1024 * 1024) });
    hold = true;
    await browser.url(`${mainUrl.split("?")[0]}?${params}`);
    await browser.$('.viewer__loading-overlay[role="status"]').waitForDisplayed({ timeout: 10_000 });
    release();
    await waitReady("initial image ready");
    sampleMemory("initial");

    hold = true;
    await browser.keys("k");
    await browser.$('.viewer__loading-overlay[role="status"]').waitForDisplayed({ timeout: 10_000 });
    release();
    await waitReady("delayed next image ready");

    for (let i = 2; i < names.length; i++) {
      measurements.forwardMs.push(await navigate("k", names[i]));
      if (i % 8 === 7) sampleMemory(`forward-${i}`);
    }
    for (let i = names.length - 2; i >= 0; i--) {
      measurements.reverseMs.push(await navigate("j", names[i]));
      if (i % 8 === 0) sampleMemory(`reverse-${i}`);
    }
    await browser.execute(() => {
      for (let i = 0; i < 12; i++) window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", bubbles: true }));
    });
    await waitFor(() => browser.execute(() => document.querySelector(".viewer__filename")?.textContent === "12.jpg"),
      { timeout: 15_000, label: "rapid navigation settles on final file" });
    await waitReady("rapid navigation image ready");
    await navigate("j", "11.jpg");
    for (let i = 12; i < names.length; i++) await navigate("k", names[i]);
    await browser.keys("k");
    await browser.$('.viewer__image-notice[role="alert"]').waitForDisplayed({ timeout: 10_000 });
    if (await browser.$(".viewer__loading-overlay").isExisting()) throw new Error("image failure left loading indicator visible");
    await navigate("j", "31.jpg");
    sampleMemory("final");
  } catch (error) {
    measurements.error = String(error);
    measurements.viewer = await browser.execute(() => ({
      filename: document.querySelector(".viewer__filename")?.textContent,
      loading: document.querySelectorAll(".viewer__loading-overlay").length,
    })).catch(() => null);
    await browser.saveScreenshot(path.join(artifactsDir, "screenshots", "viewer-failure.png")).catch(() => {});
    throw error;
  } finally {
    fs.writeFileSync(path.join(artifactsDir, "viewer-timings.json"), JSON.stringify(measurements, null, 2));
    release();
    proxy.closeAllConnections();
    await new Promise(resolve => proxy.close(resolve));
    observer.sendIntent("SetEmbeddedViewerActive", undefined, { active: false });
    await browser.url(mainUrl);
    observer.navigatePanelToPath("left", originalPath);
    await observer.waitForPanelPath("left", originalPath);
  }
}
