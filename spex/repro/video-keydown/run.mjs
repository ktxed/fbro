// Diagnose: does <video controls> swallow keydown events on each engine?
//
// For each scenario we:
//   1. Build a page with one <video> configured a specific way.
//   2. Install a window-level keydown counter (bubble) and another (capture).
//   3. Click the video player area so it gains focus / activates AVKit-equivalent.
//   4. Press j and k a few times via Playwright's keyboard.
//   5. Read back the counters.
//
// Run on each browser engine (`--browser=webkit` is the macOS-relevant one).
// On macOS, the system WKWebView and Playwright's bundled WebKit share the
// same WebCore, so AVKit interception (if it's the cause) should reproduce.
//
// Usage:
//   npm i -D playwright && npx playwright install
//   node run.mjs                       # runs all engines
//   node run.mjs --browser=webkit      # only webkit
//   node run.mjs --video=/path/to.mp4  # use a local file instead of remote
//   node run.mjs --headed              # see what's happening

import { chromium, firefox, webkit } from "playwright";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import path from "node:path";

const args = Object.fromEntries(
  process.argv.slice(2).map((a) => {
    const [k, v] = a.replace(/^-+/, "").split("=");
    return [k, v ?? true];
  }),
);

const ENGINES = args.browser ? [args.browser] : ["webkit", "chromium", "firefox"];
const HEADED = !!args.headed;

// We need a *playable* video for AVKit to actually attach. Use a tiny local
// fixture if provided, otherwise fall back to a known small public sample.
let VIDEO_SRC =
  "https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/ForBiggerBlazes.mp4";
if (args.video) {
  const abs = path.resolve(String(args.video));
  const data = readFileSync(abs).toString("base64");
  VIDEO_SRC = `data:video/mp4;base64,${data}`;
}

// Each scenario is an HTML body fragment. The element we want to test always
// has id="target". The harness clicks it, then dispatches the keys.
const SCENARIOS = [
  {
    id: "A_baseline_no_video",
    note: "sanity: no video at all, keys must reach window",
    html: `<button id="target" style="width:400px;height:200px">click me</button>`,
  },
  {
    id: "B_video_controls",
    note: "the bug case: <video controls>",
    html: `<video id="target" src="${VIDEO_SRC}" controls width="600"></video>`,
  },
  {
    id: "C_video_no_controls",
    note: "no controls at all (autoplay muted)",
    html: `<video id="target" src="${VIDEO_SRC}" autoplay muted width="600"></video>`,
  },
  {
    id: "D_video_controls_tabindex_minus_one",
    note: "controls + tabindex=-1",
    html: `<video id="target" src="${VIDEO_SRC}" controls tabindex="-1" width="600"></video>`,
  },
  {
    id: "E_video_controls_onfocus_blur",
    note: "controls + onfocus blur",
    html: `<video id="target" src="${VIDEO_SRC}" controls width="600" onfocus="this.blur()"></video>`,
  },
  {
    id: "F_video_controls_tabindex_and_onfocus_blur",
    note: "controls + tabindex=-1 + onfocus blur",
    html: `<video id="target" src="${VIDEO_SRC}" controls tabindex="-1" width="600" onfocus="this.blur()"></video>`,
  },
  {
    id: "G_video_controls_disable_pip_remote",
    note: "controls with all WebKit feature flags disabled",
    html: `<video id="target" src="${VIDEO_SRC}" controls width="600"
             disablepictureinpicture
             disableremoteplayback
             controlslist="nodownload nofullscreen noremoteplayback noplaybackrate"
             x-webkit-airplay="deny"></video>`,
  },
  {
    id: "H_video_controls_pointer_events_none_overlay",
    note: "controls + transparent overlay div with tabindex=0 absorbing focus",
    html: `<div style="position:relative;width:600px;height:340px">
             <video src="${VIDEO_SRC}" controls width="600" style="position:absolute;inset:0"></video>
             <div id="target" tabindex="0"
                  style="position:absolute;inset:0;background:transparent;outline:none"></div>
           </div>`,
  },
];

function pageHtml(bodyFragment) {
  return `<!doctype html><html><head><meta charset="utf-8"></head>
<body style="margin:0;padding:20px;font-family:monospace;background:#111;color:#eee">
  <div id="log" style="position:fixed;bottom:0;left:0;right:0;max-height:100px;overflow:auto;background:#000;padding:4px;font-size:11px"></div>
  ${bodyFragment}
  <script>
    window.__bubbleCount = 0;
    window.__captureCount = 0;
    window.__bubbleKeys = [];
    window.__captureKeys = [];
    const log = (m) => {
      const d = document.getElementById('log');
      if (d) { d.textContent += m + ' | '; d.scrollLeft = 1e9; }
    };
    window.addEventListener('keydown', (e) => {
      window.__bubbleCount++;
      window.__bubbleKeys.push(e.key);
      log('bubble:' + e.key);
    });
    window.addEventListener('keydown', (e) => {
      window.__captureCount++;
      window.__captureKeys.push(e.key);
      log('capture:' + e.key);
    }, true);
    document.addEventListener('focusin', (e) => {
      log('focusin:' + (e.target && e.target.tagName));
    });
  </script>
</body></html>`;
}

async function runEngine(engineName, engine) {
  const browser = await engine.launch({ headless: !HEADED });
  const ctx = await browser.newContext();
  const page = await ctx.newPage();

  const results = [];
  for (const sc of SCENARIOS) {
    await page.setContent(pageHtml(sc.html), { waitUntil: "domcontentloaded" });
    // Wait briefly for the video element to install its native controls.
    await page.waitForTimeout(500);

    // Click the element so it gets focus / triggers any native handoff.
    try {
      await page.click("#target", { timeout: 2000, force: true });
    } catch {
      // Some scenarios may not have a clickable #target (we still try).
    }
    // Give WebKit a beat after click so any AVKit handoff settles.
    await page.waitForTimeout(150);

    // Hit a few keys.
    for (const k of ["j", "k", "j", "k"]) await page.keyboard.press(k);
    await page.waitForTimeout(100);

    const r = await page.evaluate(() => ({
      bubble: window.__bubbleCount,
      capture: window.__captureCount,
      bubbleKeys: window.__bubbleKeys,
      captureKeys: window.__captureKeys,
      activeTag: document.activeElement && document.activeElement.tagName,
    }));
    results.push({ id: sc.id, note: sc.note, ...r });
  }
  await browser.close();
  return results;
}

const enginesMap = { chromium, firefox, webkit };

console.log("video-keydown diagnostic — Playwright");
console.log("platform:", process.platform, "arch:", process.arch);
console.log("video src:", VIDEO_SRC.slice(0, 80) + (VIDEO_SRC.length > 80 ? "..." : ""));
console.log();

for (const name of ENGINES) {
  if (!enginesMap[name]) {
    console.error("unknown browser:", name);
    process.exit(2);
  }
  console.log(`=== ${name} ===`);
  let results;
  try {
    results = await runEngine(name, enginesMap[name]);
  } catch (e) {
    console.error("engine failed:", e.message);
    continue;
  }
  // Pretty table
  const pad = (s, n) => String(s).padEnd(n);
  console.log(pad("scenario", 48), pad("bubble", 8), pad("capture", 8), pad("activeTag", 12), "note");
  console.log("-".repeat(120));
  for (const r of results) {
    const flag = r.bubble === 0 ? "❌" : r.bubble < 4 ? "⚠️ " : "✅";
    console.log(
      pad(`${flag} ${r.id}`, 48),
      pad(r.bubble, 8),
      pad(r.capture, 8),
      pad(r.activeTag || "-", 12),
      r.note,
    );
  }
  console.log();
}
