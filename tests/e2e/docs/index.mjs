// Captures the user guide's screenshots from the real app, using the smoke
// harness's Docker backends and tauri-driver (see TESTING.md for setup).
//   npm run docs:screens               # every scene
//   npm run docs:screens -- copy-dialog first-launch
import path from "node:path";
import { artifactsDir, localMode, webdriverHosts, webdriverPorts, wsUrls } from "../smoke/config.mjs";
import { startBrowser } from "../smoke/browser.mjs";
import { FbroObserver } from "../smoke/observer.mjs";
import {
  ensureArtifactDirs, ensureLocalAppBinary, requireDockerUsable, requireExecutable,
  startLocalBackends, startLocalDrivers, stopLocalBackends, stopProcesses, writeLaunchWrapper,
} from "../smoke/processes.mjs";
import { waitFor } from "../smoke/wait.mjs";
import { createDemoFixture } from "./fixture.mjs";
import { needsPeer, scenes } from "./scenes.mjs";

if (!localMode) throw new Error("docs:screens runs locally only (FBRO_E2E_LOCAL=1)");

const wanted = process.argv.slice(2);
const unknown = wanted.filter((name) => !scenes[name]);
if (unknown.length) throw new Error(`Unknown scene(s): ${unknown.join(", ")}. Known: ${Object.keys(scenes).join(", ")}`);
const selected = wanted.length ? wanted : Object.keys(scenes);

let browser;
let observer;
let peer;
let drivers = [];
try {
  requireDockerUsable();
  requireExecutable("tauri-driver");
  requireExecutable("WebKitWebDriver");
  ensureLocalAppBinary();
  ensureArtifactDirs();
  await startLocalBackends();
  drivers = await startLocalDrivers();
  browser = await startBrowser("fbro-a", webdriverHosts.a, webdriverPorts.a, writeLaunchWrapper("fbro-a", wsUrls.a));
  observer = new FbroObserver("fbro-a", wsUrls.a);
  await observer.connect();

  // The guide shows the default dark scheme at the default zoom.
  observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: "fbro-dark" });
  await waitFor(() => observer.state.settings?.general?.colorScheme === "fbro-dark", { label: "dark scheme" });
  await createDemoFixture(browser);

  // Remote scenes pair A with a second instance.
  if (selected.some((name) => needsPeer.has(name))) {
    const browserB = await startBrowser("fbro-b", webdriverHosts.b, webdriverPorts.b, writeLaunchWrapper("fbro-b", wsUrls.b));
    const observerB = new FbroObserver("fbro-b", wsUrls.b);
    await observerB.connect();
    peer = { browser: browserB, observer: observerB };
  }

  for (const name of selected) {
    console.log(`[docs] scene ${name}`);
    await scenes[name](browser, observer, peer);
  }
} catch (error) {
  if (browser) await browser.saveScreenshot(path.join(artifactsDir, "screenshots", "docs-failure.png")).catch(() => {});
  throw error;
} finally {
  peer?.observer.close();
  if (peer) await peer.browser.deleteSession().catch(() => {});
  observer?.close();
  if (browser) await browser.deleteSession().catch(() => {});
  stopProcesses(drivers);
  stopLocalBackends();
}
