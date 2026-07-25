import path from "node:path";
import {
  appBinary,
  artifactsDir,
  localMode,
  webdriverHosts,
  webdriverPorts,
  wsUrls,
} from "./config.mjs";
import { startBrowser } from "./browser.mjs";
import { FbroObserver } from "./observer.mjs";
import {
  ensureArtifactDirs,
  ensureLocalAppBinary,
  requireDockerUsable,
  requireExecutable,
  startLocalBackends,
  startLocalDrivers,
  stopLocalBackends,
  stopProcesses,
  writeFinalTree,
  writeLaunchWrapper,
} from "./processes.mjs";
import { runArchiveFlow } from "./cases/archive.mjs";
import { runFedupFlow } from "./cases/fedup.mjs";
import { runGlobalSelectionFlow } from "./cases/global-selection.mjs";
import { runGitFlow } from "./cases/git.mjs";
import { runLocalFlow } from "./cases/local.mjs";
import { runRemoteFlow } from "./cases/remote.mjs";
import { runDatetimeSearchFlow, runSearchFlow, runSizeSearchFlow } from "./cases/search.mjs";

async function main() {
  let browserA;
  let browserB;
  let observerA;
  let observerB;
  let localDrivers = [];
  let artifactsReady = false;
  try {
    if (localMode) {
      requireDockerUsable();
      requireExecutable("tauri-driver");
      requireExecutable("WebKitWebDriver");
      ensureLocalAppBinary();
      ensureArtifactDirs();
      artifactsReady = true;
      await startLocalBackends();
      localDrivers = await startLocalDrivers();
    } else {
      ensureArtifactDirs();
      artifactsReady = true;
    }

    const appA = localMode ? writeLaunchWrapper("fbro-a", wsUrls.a) : appBinary;
    const appB = localMode ? writeLaunchWrapper("fbro-b", wsUrls.b) : appBinary;

    browserA = await startBrowser("fbro-a", webdriverHosts.a, webdriverPorts.a, appA);
    observerA = new FbroObserver("fbro-a", wsUrls.a);
    await observerA.connect();

    await runLocalFlow(browserA, observerA);
    await runGlobalSelectionFlow(browserA, observerA);
    await runGitFlow(browserA, observerA);
    await runFedupFlow(browserA, observerA);
    await runArchiveFlow(browserA, observerA);
    await runSearchFlow(browserA, observerA);
    await runDatetimeSearchFlow(browserA, observerA);
    await runSizeSearchFlow(browserA, observerA);

    browserB = await startBrowser("fbro-b", webdriverHosts.b, webdriverPorts.b, appB);
    observerB = new FbroObserver("fbro-b", wsUrls.b);
    await observerB.connect();
    await runRemoteFlow(browserA, observerA, browserB, observerB);
  } catch (error) {
    if (browserA) await browserA.saveScreenshot(path.join(artifactsDir, "screenshots", "fbro-a-failure.png")).catch(() => {});
    if (browserB) await browserB.saveScreenshot(path.join(artifactsDir, "screenshots", "fbro-b-failure.png")).catch(() => {});
    throw error;
  } finally {
    if (artifactsReady) {
      writeFinalTree();
      observerA?.close();
      observerB?.close();
    }
    if (browserA) await browserA.deleteSession().catch(() => {});
    if (browserB) await browserB.deleteSession().catch(() => {});
    stopProcesses(localDrivers);
    if (localMode) stopLocalBackends();
  }
}

await main();
