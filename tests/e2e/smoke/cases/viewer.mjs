import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import { performance } from "node:perf_hooks";
import { execFileSync } from "node:child_process";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import { waitFor } from "../wait.mjs";
import { clickEntry, pressWindowKey, waitForMainWindow } from "../browser.mjs";

function minimalPdf() {
  const stream = "BT /F1 24 Tf 40 110 Td (fbro PDF preview) Tj ET\n";
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
    `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}endstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
  ];
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  for (let index = 0; index < objects.length; index++) {
    offsets.push(Buffer.byteLength(pdf));
    pdf += `${index + 1} 0 obj\n${objects[index]}\nendobj\n`;
  }
  const xref = Buffer.byteLength(pdf);
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  pdf += offsets.slice(1).map(offset => `${String(offset).padStart(10, "0")} 00000 n \n`).join("");
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return pdf;
}

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
  // Real 32×32 HEIC encoded by macOS sips; HEIF is the same container family.
  const heic = fs.readFileSync(new URL("../fixtures/viewer.heic", import.meta.url));
  const heicNames = ["HEIC preview.heic", "HEIF preview.HEIF"];
  for (const name of heicNames) fs.writeFileSync(hostPath(`${directory}/${name}`), heic);
  const pdfName = "PDF preview.pdf";
  const audioName = "MP3 preview.MP3";
  // Renderer/keyboard fixture; playback is mocked below, not a codec test.
  fs.writeFileSync(hostPath(`${directory}/${audioName}`), "ID3");
  fs.writeFileSync(hostPath(`${directory}/${pdfName}`), minimalPdf());
  const afterPdfName = "ZZ after PDF.txt";
  fs.writeFileSync(hostPath(`${directory}/${afterPdfName}`), "Viewer title follows navigation. " + "wide text ".repeat(50) + "\n");
  const videoName = "ZZZZ Video seek.mp4";
  const video = fs.readFileSync(new URL("../fixtures/viewer.mp4", import.meta.url));
  fs.writeFileSync(hostPath(`${directory}/${videoName}`), video);
  const largeName = "ZZZZZ Large.txt";
  fs.writeFileSync(hostPath(`${directory}/${largeName}`), "large file line " + "wide text ".repeat(50) + "\n"
    + "large file line\n".repeat(90_000) + "END OF LARGE PREVIEW\n");
  const binaryName = "ZZZZZZ Binary.bin";
  fs.writeFileSync(hostPath(`${directory}/${binaryName}`), Buffer.alloc(2048, 0));
  const linkName = "ZZZZZZZ video-link";
  const linkPath = hostPath(`${directory}/${linkName}`);
  if (fs.existsSync(linkPath)) fs.unlinkSync(linkPath);
  fs.symlinkSync(videoName, linkPath);
  fs.mkdirSync(hostPath(`${directory}/Preview folder`), { recursive: true });
  const mermaidName = "ZZZ Mermaid state.md";
  const mermaidMarkdown = `# Mermaid state diagram

![Local preview image](00.jpg)

<p align="center">
  <img src="01.jpg" alt="HTML preview image" width="96" height="96">
</p>

\`\`\`mermaid
stateDiagram-v2
    [*] --> PENDING : confirmed request,<br/>catalogue says invoiceAutomatically

    note right of PENDING
        ON_DEMAND mode only picks up rows
        with released_at set.
    end note
\`\`\`
`;
  fs.writeFileSync(hostPath(`${directory}/${mermaidName}`), mermaidMarkdown);

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
      `/__thumb__/128/%2Fproc%2Fself%2Fmountinfo?prefetch=1&panelId=left&panelEpoch=${observer.state.panels.left.panelEpoch}`,
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
    const sortEpoch = observer.state.panels.left.panelEpoch;
    observer.sendIntent("SetSort", "left", { by: "name", order: "asc" });
    await waitFor(() => {
      const panel = observer.state.panels.left;
      return panel.panelEpoch !== sortEpoch && panel.status === "ready" && panel.entries.some(entry => entry.name === names[0]);
    }, { timeout: 10_000, label: "viewer sorted listing completes before setting cursor" });
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
    await browser.keys("j");
    await browser.$('.viewer__loading-overlay[role="status"]').waitForDisplayed({ timeout: 10_000 });
    release();
    await waitReady("delayed next image ready");

    for (let i = 2; i < names.length; i++) {
      measurements.forwardMs.push(await navigate("j", names[i]));
      if (i % 8 === 7) sampleMemory(`forward-${i}`);
    }
    for (let i = names.length - 2; i >= 0; i--) {
      measurements.reverseMs.push(await navigate("k", names[i]));
      if (i % 8 === 0) sampleMemory(`reverse-${i}`);
    }
    await browser.execute(() => {
      for (let i = 0; i < 12; i++) document.querySelector("[data-viewer-content]").dispatchEvent(new KeyboardEvent("keydown", { key: "j", bubbles: true, cancelable: true }));
    });
    await waitFor(() => browser.execute(() => document.querySelector(".viewer__filename")?.textContent === "12.jpg"),
      { timeout: 15_000, label: "rapid navigation settles on final file" });
    await waitReady("rapid navigation image ready");
    await navigate("k", "11.jpg");
    for (let i = 12; i < names.length; i++) await navigate("j", names[i]);
    await browser.keys("j");
    await browser.$('.viewer__image-notice[role="alert"]').waitForDisplayed({ timeout: 10_000 });
    if (await browser.$(".viewer__loading-overlay").isExisting()) throw new Error("image failure left loading indicator visible");
    await navigate("k", "31.jpg");
    sampleMemory("final");

    const isMac = await browser.execute(() =>
      navigator.platform.startsWith("Mac") || navigator.userAgent.includes("Mac"));
    for (const name of heicNames) {
      const imagePath = `${directory}/${name}`;
      const response = await fetch(`http://127.0.0.1:${observer.state.fileServerPort}${imagePath.split("/").map(encodeURIComponent).join("/")}`);
      const expectedMime = name.endsWith("heic") ? "image/heic" : "image/heif";
      if (response.headers.get("content-type") !== expectedMime
        || !Buffer.from(await response.arrayBuffer()).equals(heic)) {
        throw new Error(`HEIC/HEIF HTTP response differs from fixture: ${name}`);
      }
      // Exercise HTTP and ReadFileChunk/Blob paths. The local flag only selects
      // the renderer path; both reads still use the real local backend source.
      for (const local of ["1", "0"]) {
        const imageParams = new URLSearchParams({ viewer: "1", path: imagePath,
          source: observer.state.panels.left.sourceId, name, label: "main", panel: "left",
          local, size: String(heic.length), fsport: String(observer.state.fileServerPort) });
        await browser.url(`${mainUrl.split("?")[0]}?${imageParams}`);
        if (isMac) {
          await waitReady(`HEIC/HEIF ${local === "1" ? "HTTP" : "Blob"} preview ${name}`);
          const dimensions = await browser.execute(() => {
            const image = document.querySelector(".viewer__image-container > img");
            return [image.naturalWidth, image.naturalHeight];
          });
          if (dimensions[0] !== 32 || dimensions[1] !== 32) {
            throw new Error(`Incorrect HEIC dimensions: ${dimensions}`);
          }
        } else {
          await waitFor(() => browser.execute(() =>
            document.querySelector(".viewer__mode-btn--active")?.textContent?.includes("Hex")), {
            timeout: 10_000, label: `HEIC/HEIF hex fallback ${name}`,
          });
        }
      }
    }

    await observer.waitForEntry("left", afterPdfName);
    const pdfIndex = observer.state.panels.left.entries.findIndex(entry => entry.name === pdfName);
    if (pdfIndex < 0) throw new Error("PDF fixture is missing from the viewer directory");
    observer.sendIntent("SetCursor", "left", { index: pdfIndex });
    await waitFor(() => observer.state.panels.left.cursor === pdfIndex, {
      timeout: 10_000,
      label: "PDF fixture cursor",
    });
    const pdfParams = new URLSearchParams({ viewer: "1", path: `${directory}/${pdfName}`,
      source: observer.state.panels.left.sourceId, name: pdfName, label: "main", panel: "left",
      local: "1", size: String(fs.statSync(hostPath(`${directory}/${pdfName}`)).size),
      fsport: String(observer.state.fileServerPort) });
    await browser.url(`${mainUrl.split("?")[0]}?${pdfParams}`);
    await browser.$('.viewer__pdf-page:first-child[data-rendered="true"] canvas').waitForDisplayed({ timeout: 15_000 });
    if (await browser.$(".viewer__pdf-error").isExisting()) throw new Error("PDF viewer reported an error");
    await browser.keys("j");
    await waitFor(() => browser.execute(expected =>
      document.querySelector(".viewer__filename")?.textContent === expected, afterPdfName), {
      timeout: 10_000,
      label: "navigation away from PDF",
    });
    await waitFor(async () => (await browser.getTitle()) === `${afterPdfName} — fbro viewer`, {
      timeout: 10_000,
      label: "viewer window title after PDF navigation",
    });
    await checkHorizontalScrolling(browser, "small text");
    await browser.keys("f");
    await browser.$(".viewer__find-bar input").setValue("wide");
    await browser.keys("ArrowLeft");
    const findState = await browser.execute(() => ({
      cursor: document.querySelector(".viewer__find-bar input").selectionStart,
      scroll: document.querySelector(".viewer__text").scrollLeft,
    }));
    if (findState.cursor !== 3 || findState.scroll !== 0) {
      throw new Error(`Find input lost arrow-key ownership: ${JSON.stringify(findState)}`);
    }
    await checkFindNavigation(browser, "small text");

    const largeParams = new URLSearchParams({ viewer: "1", path: `${directory}/${largeName}`,
      source: observer.state.panels.left.sourceId, name: largeName, label: "main", panel: "left",
      local: "1", size: String(fs.statSync(hostPath(`${directory}/${largeName}`)).size),
      fsport: String(observer.state.fileServerPort) });
    await browser.url(`${mainUrl.split("?")[0]}?${largeParams}`);
    await browser.$(".viewer__text--virtual").waitForDisplayed({ timeout: 10_000 });
    await checkHorizontalScrolling(browser, "large text");
    await browser.keys("f");
    await browser.$(".viewer__find-bar input").setValue("wide");
    await checkFindNavigation(browser, "large text");

    const audioParams = new URLSearchParams({ viewer: "1", path: `${directory}/${audioName}`,
      source: observer.state.panels.left.sourceId, name: audioName, label: "main", panel: "left",
      local: "1", size: "3", fsport: String(observer.state.fileServerPort) });
    await browser.url(`${mainUrl.split("?")[0]}?${audioParams}`);
    await browser.$('[data-testid="viewer-audio"]').waitForDisplayed({ timeout: 10_000 });
    await browser.execute(() => {
      const audio = document.querySelector('[data-testid="viewer-audio"]');
      let paused = true;
      audio.dataset.playCalls = "0";
      audio.dataset.pauseCalls = "0";
      Object.defineProperty(audio, "paused", { get: () => paused });
      audio.play = () => {
        paused = false;
        audio.dataset.playCalls = String(Number(audio.dataset.playCalls) + 1);
        return Promise.resolve();
      };
      audio.pause = () => {
        paused = true;
        audio.dataset.pauseCalls = String(Number(audio.dataset.pauseCalls) + 1);
      };
    });
    await browser.keys("Space");
    await browser.keys("Space");
    const audioKeys = await browser.execute(() => {
      const audio = document.querySelector('[data-testid="viewer-audio"]');
      return { play: audio.dataset.playCalls, pause: audio.dataset.pauseCalls, controls: audio.controls };
    });
    if (audioKeys.play !== "1" || audioKeys.pause !== "1" || !audioKeys.controls) {
      throw new Error(`MP3 keyboard playback failed: ${JSON.stringify(audioKeys)}`);
    }

    await observer.waitForEntry("left", mermaidName);
    const videoParams = new URLSearchParams({ viewer: "1", path: `${directory}/${videoName}`,
      source: observer.state.panels.left.sourceId, name: videoName, label: "main", panel: "left",
      local: "1", size: String(video.length), fsport: String(observer.state.fileServerPort) });
    await browser.url(`${mainUrl.split("?")[0]}?${videoParams}`);
    await checkRealVideoSeeking(browser, "full viewer");

    const mermaidParams = new URLSearchParams({ viewer: "1", path: `${directory}/${mermaidName}`,
      source: observer.state.panels.left.sourceId, name: mermaidName, label: "main", panel: "left",
      local: "1", size: String(Buffer.byteLength(mermaidMarkdown)),
      fsport: String(observer.state.fileServerPort) });
    await browser.url(`${mainUrl.split("?")[0]}?${mermaidParams}`);
    await browser.$(".viewer__mermaid-canvas > svg").waitForDisplayed({ timeout: 15_000 });
    await browser.waitUntil(() => browser.execute(() => {
      const image = document.querySelector('.viewer__markdown img[alt="Local preview image"]');
      return image?.complete && image.naturalWidth > 0;
    }), { timeout: 15_000, timeoutMsg: "Markdown local image did not load" });
    await browser.waitUntil(() => browser.execute(() => {
      const image = document.querySelector('.viewer__markdown img[alt="HTML preview image"]');
      return image?.complete && image.naturalWidth > 0
        && image.width === 96 && image.height === 96
        && getComputedStyle(image.parentElement).textAlign === "center";
    }), { timeout: 15_000, timeoutMsg: "Centered HTML image did not load with its dimensions" });
    const mermaidResult = await browser.execute(() => ({
      diagrams: document.querySelectorAll(".viewer__mermaid-canvas > svg").length,
      rawBlocks: document.querySelectorAll("code.language-mermaid").length,
      text: document.querySelector(".viewer__mermaid-canvas > svg")?.textContent ?? "",
    }));
    if (mermaidResult.diagrams !== 1 || mermaidResult.rawBlocks !== 0
      || !mermaidResult.text.includes("ON_DEMAND mode")
      || !mermaidResult.text.includes("catalogue says")
      || !mermaidResult.text.includes("invoiceAutomatically")) {
      throw new Error(`Mermaid state diagram did not render correctly: ${JSON.stringify(mermaidResult)}`);
    }

    // Inline zoom: the toolbar is hover-revealed, so drive its buttons directly.
    const mermaidZoom = await browser.execute(() => {
      const svg = document.querySelector(".viewer__mermaid-canvas > svg");
      const before = svg?.getBoundingClientRect().width ?? 0;
      document.querySelector('.viewer__mermaid [data-mermaid-action="in"]')?.click();
      document.querySelector('.viewer__mermaid [data-mermaid-action="in"]')?.click();
      const zoomed = svg?.getBoundingClientRect().width ?? 0;
      document.querySelector('.viewer__mermaid [data-mermaid-action="fit"]')?.click();
      const fitted = svg?.getBoundingClientRect().width ?? 0;
      return { before, zoomed, fitted };
    });
    if (!(mermaidZoom.zoomed > mermaidZoom.before) || Math.abs(mermaidZoom.fitted - mermaidZoom.before) > 1) {
      throw new Error(`Mermaid inline zoom did not resize the diagram: ${JSON.stringify(mermaidZoom)}`);
    }

    // Expand overlay: Escape returns to the document without closing the viewer.
    await browser.execute(() => {
      document.querySelector('.viewer__mermaid [data-mermaid-action="expand"]')?.click();
    });
    await browser.$(".viewer__mermaid-overlay-stage > svg").waitForDisplayed({ timeout: 5_000 });
    await browser.keys("Escape");
    await waitFor(async () => browser.execute(() =>
      !document.querySelector(".viewer__mermaid-overlay")
      && document.querySelectorAll(".viewer__mermaid-canvas > svg").length === 1), {
      timeout: 5_000,
      label: "mermaid overlay closed by Escape",
    });
    await runQuickPreviewChecks(browser, observer, {
      mainUrl, directory, videoName, linkName, largeName, binaryName, mermaidName, pdfName,
    });
  } catch (error) {
    measurements.error = String(error);
    measurements.viewer = await browser.execute(() => ({
      filename: document.querySelector(".viewer__filename")?.textContent,
      focus: document.activeElement?.outerHTML.slice(0, 600),
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

// Catches swallowed Enter in inputs, missing content shortcuts, double native
// button activation, and history selection accidentally advancing a match.
async function checkFindNavigation(browser, label) {
  const expectMatch = async (index, focusSelector) => {
    await waitFor(() => browser.execute(({ index, focusSelector }) => {
      const count = document.querySelector(".viewer__find-count")?.textContent ?? "";
      return (count === `${index} / 50` || count === `${index} / 50 · line 1`)
        && document.activeElement === document.querySelector(focusSelector)
        && document.querySelectorAll(".viewer__find-match--active").length === 1;
    }, { index, focusSelector }), { timeout: 5000, label: `${label} find match ${index}, focus ${focusSelector}` });
  };
  const input = '[data-testid="viewer-find-input"]';
  const content = "[data-viewer-content]";
  await expectMatch(1, input);
  for (const focusSelector of [input, content]) {
    await browser.execute(selector => document.querySelector(selector).focus(), focusSelector);
    await browser.keys("Enter");
    await expectMatch(2, focusSelector);
    await browser.keys(["Shift", "Enter"]);
    await expectMatch(1, focusSelector);
    await browser.keys(["Shift", "Enter"]);
    await expectMatch(50, focusSelector);
    await browser.keys("Enter");
    await expectMatch(1, focusSelector);
  }

  const nextButton = '[data-testid="viewer-find-next"]';
  const previousButton = '[data-testid="viewer-find-previous"]';
  await browser.execute(selector => document.querySelector(selector).focus(), nextButton);
  await browser.keys("Enter");
  await expectMatch(2, nextButton);
  await browser.execute(selector => document.querySelector(selector).focus(), previousButton);
  await browser.keys("Enter");
  await expectMatch(1, previousButton);

  await browser.$(input).setValue("text");
  await browser.keys(["Alt", "ArrowDown", "Alt"]);
  await browser.$(".input-history").waitForDisplayed();
  await browser.keys("ArrowDown");
  await browser.keys("Enter");
  await browser.$(".input-history").waitForDisplayed({ reverse: true });
  if (await browser.$(input).getValue() !== "wide") throw new Error(`${label}: find history lost the current query`);
  await expectMatch(1, input);

  // Escape dismisses history before find, then find restores its content opener.
  await browser.keys(["Alt", "ArrowDown", "Alt"]);
  await browser.$(".input-history").waitForDisplayed();
  await browser.keys("Escape");
  await browser.$(".input-history").waitForDisplayed({ reverse: true });
  await expectMatch(1, input);
  await browser.$(input).setValue("no-such-viewer-match");
  await waitFor(async () => await browser.$(".viewer__find-count").getText() === "No matches",
    { timeout: 5000, label: `${label} empty find results ready` });
  await browser.keys("Enter");
  await browser.keys(["Shift", "Enter"]);
  if (await browser.$(".viewer__find-count").getText() !== "No matches") throw new Error(`${label}: empty find results changed`);
  await browser.keys("Escape");
  await browser.$(".viewer__find-bar").waitForDisplayed({ reverse: true });
  await waitFor(() => browser.execute(() => document.activeElement === document.querySelector("[data-viewer-content]")),
    { timeout: 3000, label: `${label} find restores content focus` });
}

async function checkHorizontalScrolling(browser, label) {
  const overflows = await browser.execute(() => {
    const surface = document.querySelector(".viewer__text");
    document.querySelector("[data-viewer-content]").focus();
    surface.scrollLeft = 0;
    return surface.scrollWidth > surface.clientWidth + 160;
  });
  if (!overflows) throw new Error(`${label} fixture does not overflow horizontally`);
  await browser.keys("ArrowRight");
  await browser.keys("ArrowRight");
  await waitFor(() => browser.execute(() => document.querySelector(".viewer__text").scrollLeft >= 160),
    { timeout: 3000, label: `${label} scrolls right from viewer root` });
  await browser.keys("ArrowLeft");
  await browser.keys("ArrowLeft");
  await waitFor(() => browser.execute(() => document.querySelector(".viewer__text").scrollLeft === 0),
    { timeout: 3000, label: `${label} scrolls left from viewer root` });
}

async function checkRealVideoSeeking(browser, label) {
  await waitFor(() => browser.execute(() => {
    const video = document.querySelector("video");
    return !!video && Number.isFinite(video.duration) && video.duration >= 14 && video.readyState >= 1;
  }), { timeout: 15_000, label: `${label}: real clip metadata` });
  await browser.execute(() => {
    const video = document.querySelector("video");
    video.pause();
    video.currentTime = 2;
    document.querySelector("[data-viewer-content]").focus();
  });
  await browser.keys("ArrowRight");
  await waitFor(() => browser.execute(() => Math.abs(document.querySelector("video").currentTime - 7) < 0.2),
    { timeout: 5000, label: `${label}: right seeks exactly five seconds` });
  await browser.keys("ArrowLeft");
  await waitFor(() => browser.execute(() => Math.abs(document.querySelector("video").currentTime - 2) < 0.2),
    { timeout: 5000, label: `${label}: left seeks exactly five seconds` });
  const pointer = await browser.execute(() => {
    const video = document.querySelector("video");
    const event = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    video.dispatchEvent(event);
    return { cancelled: event.defaultPrevented, controls: video.controls };
  });
  if (pointer.cancelled || !pointer.controls) throw new Error(`${label}: native media pointer controls blocked`);
  // WebKit shows only its central Play overlay until playback starts once.
  await browser.$("video").click();
  await waitFor(() => browser.execute(() => !document.querySelector("video").paused),
    { timeout: 5000, label: `${label}: native Play responds to mouse` });
  await browser.execute(() => { const video = document.querySelector("video"); video.pause(); video.currentTime = 2; });
  const rect = await browser.execute(() => {
    const bounds = document.querySelector("video").getBoundingClientRect();
    return { x: Math.round(bounds.left + bounds.width * 0.6), y: Math.round(bounds.bottom - 16) };
  });
  await browser.performActions([{
    type: "pointer", id: "media-slider", parameters: { pointerType: "mouse" }, actions: [
      { type: "pointerMove", duration: 0, origin: "viewport", x: rect.x, y: rect.y },
      { type: "pause", duration: 500 },
      { type: "pointerDown", button: 0 }, { type: "pointerUp", button: 0 },
    ],
  }]);
  await browser.releaseActions();
  await waitFor(() => browser.execute(() => {
    const video = document.querySelector("video");
    return video.paused && video.currentTime > 3;
  }), { timeout: 5000, label: `${label}: native slider seeks with mouse` });
  await browser.execute(() => document.querySelector("video").currentTime = 2);
}

async function runQuickPreviewChecks(browser, observer, fixture) {
  const { mainUrl, directory, videoName, linkName, largeName, binaryName, mermaidName, pdfName } = fixture;
  await browser.url(mainUrl);
  await waitForMainWindow(browser, "quick preview");
  await observer.waitForEntry("left", videoName);
  const choose = async (name, displayed = name) => {
    const oldRequest = await browser.execute(() => document.querySelector(".embedded-viewer")?.dataset.previewRequestId);
    const index = observer.state.panels.left.entries.findIndex(entry => entry.name === name);
    if (index < 0) throw new Error(`Quick preview fixture missing: ${name}`);
    await clickEntry(browser, "left", name);
    await waitFor(() => browser.execute((expected, previousRequest) =>
      document.querySelector(".embedded-viewer")?.dataset.previewRequestId !== previousRequest
      && document.querySelector(".embedded-viewer__header-label")?.textContent.trim() === expected, displayed, oldRequest),
    { timeout: 10_000, label: `preview target ${displayed}` });
  };
  await clickEntry(browser, "left", videoName);
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]').focus());
  await pressWindowKey(browser, "F3", { ctrl: true });
  await browser.$('[data-testid="preview-right"] video').waitForDisplayed({ timeout: 10_000 });
  if (!(await browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"))) {
    throw new Error("Ctrl+F3 stole source-list focus");
  }
  await browser.keys("Tab");
  await waitFor(() => browser.execute(() => document.activeElement?.dataset.viewerContent === "quick"),
    { timeout: 5000, label: "Tab enters quick preview content" });
  await checkRealVideoSeeking(browser, "quick preview");
  // The source listing delegates Space to the displayed video, preserving its
  // cursor, selection and focus. Repeated keydowns must not toggle it again.
  const sourceListingState = () => browser.execute(() => {
    const panel = document.querySelector('[data-testid="panel-left"]');
    return JSON.stringify({
      cursor: panel.querySelector(".panel__row--cursor")?.dataset.absIndex,
      selection: Array.from(panel.querySelectorAll(".panel__row--selected")).map(row => row.dataset.absIndex),
    });
  });
  const sourceBeforePlayback = await sourceListingState();
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]').focus());
  await browser.keys("Space");
  await waitFor(() => browser.execute(() => !document.querySelector("video").paused),
    { timeout: 5000, label: "source-list Space starts preview video" });
  const repeatedSpace = await browser.execute(() => {
    const event = new KeyboardEvent("keydown", { key: " ", code: "Space", repeat: true, bubbles: true, cancelable: true });
    document.activeElement.dispatchEvent(event);
    return { consumed: event.defaultPrevented, playing: !document.querySelector("video").paused };
  });
  if (!repeatedSpace.consumed || !repeatedSpace.playing) throw new Error("Held Space toggled preview playback again");
  await browser.keys("Space");
  await waitFor(() => browser.execute(() => document.querySelector("video").paused),
    { timeout: 5000, label: "source-list Space pauses preview video" });
  if (!(await browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"))
    || sourceBeforePlayback !== await sourceListingState()) {
    throw new Error("Video Space changed source-list focus/cursor/selection");
  }
  // A modal owns the same arrow key while the preview remains covered.
  await browser.execute(() => document.querySelector('[data-viewer-content="quick"]').focus());
  await pressWindowKey(browser, ",", { ctrl: true });
  await browser.$('[data-modal-id="settings"]').waitForDisplayed();
  const beforeModalKey = await browser.execute(() => document.querySelector("video").currentTime);
  await browser.keys("ArrowRight");
  await browser.keys("Space");
  const duringModal = await browser.execute(() => document.querySelector("video").currentTime);
  if (Math.abs(duringModal - beforeModalKey) > 0.1
    || !(await browser.execute(() => document.querySelector("video").paused))) throw new Error("Covered preview handled modal media keys");
  await browser.keys("Escape");
  await browser.$('[data-modal-id="settings"]').waitForDisplayed({ reverse: true });
  await waitFor(() => browser.execute(() => document.activeElement?.dataset.viewerContent === "quick"),
    { timeout: 5000, label: "modal restores its quick-preview opener" });
  await browser.execute(() => document.querySelector('[data-testid="preview-close"]').focus());
  await browser.keys("Tab");
  await waitFor(() => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"),
    { timeout: 5000, label: "last preview control returns Tab to source listing" });
  await browser.keys("Tab");
  await waitFor(() => browser.execute(() => document.activeElement?.dataset.viewerContent === "quick"),
    { timeout: 5000, label: "source Tab re-enters preview" });
  if (await browser.$(".embedded-viewer .viewer__mode-btns").isExisting()) {
    throw new Error("Quick preview contains full-viewer options");
  }

  // Observe the real outgoing intent while retaining normal backend dispatch.
  await browser.execute(() => {
    window.__previewIntents = [];
    const send = WebSocket.prototype.send;
    WebSocket.prototype.send = function(data) {
      if (typeof data === "string") {
        const message = JSON.parse(data);
        if (message.type === "Intent") window.__previewIntents.push(message);
      }
      return send.call(this, data);
    };
  });
  await browser.keys(["j", "k", "m", "e", "w", "s", "f", "1", "2", "3", "4"]);
  const excluded = await browser.execute(() => ({
    find: !!document.querySelector(".embedded-viewer .viewer__find-bar"),
    video: !!document.querySelector(".embedded-viewer video"),
    commands: window.__previewIntents.filter(message => ["ViewerNavigate", "EditFile"].includes(message.name)),
  }));
  if (excluded.find || !excluded.video || excluded.commands.length) {
    throw new Error(`Quick preview accepted excluded actions: ${JSON.stringify(excluded)}`);
  }
  await browser.keys("F3");
  await waitFor(() => browser.execute(() => window.__previewIntents.some(message => message.name === "ViewFile")),
    { timeout: 5000, label: "F3 promotes displayed preview file" });
  const promotion = await browser.execute(() => window.__previewIntents.find(message => message.name === "ViewFile").args);
  if (promotion.filePath !== `${directory}/${videoName}` || promotion.panelId !== "left") {
    throw new Error(`Open viewer targeted the hidden panel: ${JSON.stringify(promotion)}`);
  }
  if (!(await browser.$('[data-testid="preview-right"]').isExisting())) throw new Error("Open viewer removed quick preview");
  await browser.$('[data-testid="preview-open-viewer"]').click();
  const openings = await browser.execute(() => window.__previewIntents.filter(message => message.name === "ViewFile"));
  if (openings.length !== 2) throw new Error("Open viewer button did not dispatch exactly once");

  await choose(linkName, videoName);
  await checkRealVideoSeeking(browser, "symlink quick preview");
  await choose(largeName);
  await waitFor(() => browser.execute(() => document.querySelector(".embedded-viewer .viewer__content")?.textContent.includes("large file line")),
    { timeout: 10_000, label: "large text loaded in preview" });
  const textWasSelected = await browser.execute(() => document.querySelector('[data-testid="panel-left"] .panel__row--cursor').classList.contains("panel__row--selected"));
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]').focus());
  await browser.keys("Space");
  await waitFor(async () => await browser.execute(() => document.querySelector('[data-testid="panel-left"] .panel__row--cursor').classList.contains("panel__row--selected")) !== textWasSelected,
    { timeout: 5000, label: "non-video preview keeps source-list Space selection" });
  await browser.execute(() => document.querySelector('[data-viewer-content="quick"]').focus());
  await browser.keys(["w", "s", "f", "3"]);
  if (await browser.$(".embedded-viewer .viewer__find-bar, .embedded-viewer .viewer--dos, .embedded-viewer .viewer__text--wrap").isExisting()) {
    throw new Error("Quick text preview enabled a full-viewer transformation");
  }
  await browser.keys("End");
  await waitFor(() => browser.execute(() => document.querySelector(".embedded-viewer .viewer__content")?.textContent.includes("END OF LARGE PREVIEW")),
    { timeout: 10_000, label: "large preview reaches content beyond 1 MiB" });
  await choose(binaryName);
  await waitFor(() => browser.execute(() => document.querySelector(".embedded-viewer .viewer__pre")?.textContent.includes("000007f0")),
    { timeout: 10_000, label: "binary preview includes bytes beyond old 64-row limit" });
  await choose(pdfName);
  await browser.$('.embedded-viewer .viewer__pdf-page:first-child[data-rendered="true"] canvas').waitForDisplayed({ timeout: 15_000 });
  await browser.execute(() => document.querySelector('[data-viewer-content="quick"]').focus());
  await browser.keys("End");
  await waitFor(() => browser.execute(() => document.querySelector(".embedded-viewer .viewer__pdf").scrollTop > 0),
    { timeout: 5000, label: "PDF keyboard scroll in preview" });
  await choose(mermaidName);
  await browser.$(".embedded-viewer .viewer__mermaid-canvas > svg").waitForDisplayed({ timeout: 15_000 });
  await browser.execute(() => {
    document.querySelector('[data-viewer-content="quick"]').focus();
    document.querySelector('.embedded-viewer [data-mermaid-action="expand"]').click();
  });
  await browser.$(".embedded-viewer .viewer__mermaid-overlay").waitForDisplayed();
  const beforeDiagramKey = await browser.execute(() => window.__previewIntents.filter(message => message.name === "ViewFile").length);
  await browser.keys("F3");
  const afterDiagramKey = await browser.execute(() => window.__previewIntents.filter(message => message.name === "ViewFile").length);
  if (afterDiagramKey !== beforeDiagramKey) throw new Error("Expanded diagram forwarded F3 to the covered preview");
  await browser.keys("Escape");
  await browser.$(".embedded-viewer .viewer__mermaid-overlay").waitForDisplayed({ reverse: true });
  if (!(await browser.$('[data-testid="preview-right"]').isExisting())) throw new Error("Diagram Escape closed preview");

  await choose("Preview folder");
  if (!(await browser.$('[data-testid="preview-open-viewer"]').getAttribute("disabled"))) {
    throw new Error("Directory preview offers Open viewer");
  }
  await browser.execute(() => document.querySelector('[data-testid="preview-right"]').focus());
  await browser.keys("Escape");
  await browser.$('[data-testid="preview-right"]').waitForDisplayed({ reverse: true });
  await waitFor(() => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"),
    { timeout: 5000, label: "closing preview restores source-list focus" });
}
