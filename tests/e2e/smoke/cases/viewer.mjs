import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import { performance } from "node:perf_hooks";
import { execFileSync } from "node:child_process";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import { waitFor } from "../wait.mjs";

function minimalPdf() {
  const stream = "BT /F1 24 Tf 40 110 Td (fbro PDF preview) Tj ET\n";
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
    `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}endstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
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
  fs.writeFileSync(hostPath(`${directory}/${afterPdfName}`), "Viewer title follows navigation.\n");
  const mermaidName = "ZZZ Mermaid state.md";
  const mermaidMarkdown = `# Mermaid state diagram

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
    await browser.$('.viewer__pdf-page[data-rendered="true"] canvas').waitForDisplayed({ timeout: 15_000 });
    if (await browser.$(".viewer__pdf-error").isExisting()) throw new Error("PDF viewer reported an error");
    await browser.keys("k");
    await waitFor(() => browser.execute(expected =>
      document.querySelector(".viewer__filename")?.textContent === expected, afterPdfName), {
      timeout: 10_000,
      label: "navigation away from PDF",
    });
    await waitFor(async () => (await browser.getTitle()) === `${afterPdfName} — fbro viewer`, {
      timeout: 10_000,
      label: "viewer window title after PDF navigation",
    });

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
    const mermaidParams = new URLSearchParams({ viewer: "1", path: `${directory}/${mermaidName}`,
      source: observer.state.panels.left.sourceId, name: mermaidName, label: "main", panel: "left",
      local: "1", size: String(Buffer.byteLength(mermaidMarkdown)),
      fsport: String(observer.state.fileServerPort) });
    await browser.url(`${mainUrl.split("?")[0]}?${mermaidParams}`);
    await browser.$(".viewer__mermaid-canvas > svg").waitForDisplayed({ timeout: 15_000 });
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
