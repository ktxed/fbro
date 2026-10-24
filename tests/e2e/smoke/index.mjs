import path from "node:path";
import {
  appBinary,
  artifactsDir,
  backendPath,
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
import { runHashGenerateFlow } from "./cases/hash-generate.mjs";
import { runHashCheckFlow } from "./cases/hash-check.mjs";
import { runGlobalSelectionFlow } from "./cases/global-selection.mjs";
import { runGitFlow } from "./cases/git.mjs";
import { runGitInitFlow } from "./cases/git-init.mjs";
import { runLocalFlow, verifyBookmarksKeyboardNavigation } from "./cases/local.mjs";
import { runPanelSelectionFlow } from "./cases/panel-selection.mjs";
import { runSelectionMaskFlow } from "./cases/selection-mask.mjs";
import { runFocusHandoffFlow } from "./cases/focus-handoff.mjs";
import { runNotificationsFlow } from "./cases/notifications.mjs";
import { runViewerFlow } from "./cases/viewer.mjs";
import { runDiffFlow } from "./cases/diff.mjs";
import { runHeicFlow } from "./cases/heic.mjs";
import { runButtonBarFlow } from "./cases/button-bar.mjs";
import { runTabsFlow } from "./cases/tabs.mjs";
import { runQuickEditFlow } from "./cases/quick-edit.mjs";
import { runJournalSortFlow } from "./cases/journal-sort.mjs";
import { verifyThumbnailQueue } from "./cases/thumbnails.mjs";
import { runRemoteFlow } from "./cases/remote.mjs";
import { runDatetimeSearchFlow, runSearchActionsFlow, runSearchActivityFlow, runSearchFlow, runSizeSearchFlow } from "./cases/search.mjs";

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

    if (process.env.FBRO_E2E_JOURNAL_SORT_ONLY === "1") {
      await runJournalSortFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_TABS_ONLY === "1") {
      await runTabsFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_BOOKMARKS_ONLY === "1") {
      const aSrc = backendPath("a", "src");
      await verifyBookmarksKeyboardNavigation(browserA, observerA, aSrc, aSrc);
      return;
    }

    if (process.env.FBRO_E2E_BUTTON_BAR_ONLY === "1") {
      await runButtonBarFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_PANEL_SELECTION_ONLY === "1") {
      await runPanelSelectionFlow(browserA, observerA);
      await runSelectionMaskFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_SELECTION_MASK_ONLY === "1") {
      await runSelectionMaskFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_NOTIFICATIONS_ONLY === "1") {
      await runNotificationsFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_THUMBNAILS_ONLY === "1") {
      await verifyThumbnailQueue(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_REMOTE_ONLY === "1") {
      browserB = await startBrowser("fbro-b", webdriverHosts.b, webdriverPorts.b, appB);
      observerB = new FbroObserver("fbro-b", wsUrls.b);
      await observerB.connect();
      await runRemoteFlow(browserA, observerA, browserB, observerB);
      return;
    }

    if (process.env.FBRO_E2E_SEARCH_ACTIVITY_ONLY === "1") {
      await runSearchActivityFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_SEARCH_ACTIONS_ONLY === "1") {
      await runSearchActionsFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_DIFF_ONLY === "1") {
      await runDiffFlow(browserA, observerA);
      return;
    }
    if (process.env.FBRO_E2E_GIT_ONLY === "1") {
      await runGitInitFlow(browserA, observerA);
      await runGitFlow(browserA, observerA);
      return;
    }

    if (process.env.FBRO_E2E_HASH_GENERATE_ONLY === "1") {
      await runHashGenerateFlow(browserA, observerA);
      await runHashCheckFlow(browserA, observerA);
      return;
    }
    if (process.env.FBRO_E2E_HASH_CHECK_ONLY === "1") {
      await runHashCheckFlow(browserA, observerA);
      return;
    }
    if (process.env.FBRO_E2E_VIEWER_ONLY === "1" || process.argv.includes("--viewer-only")) {
      await runViewerFlow(browserA, observerA);
      return;
    }
    await runNotificationsFlow(browserA, observerA);
    await runButtonBarFlow(browserA, observerA);
    await runFocusHandoffFlow(browserA, observerA);
    if (process.env.FBRO_E2E_FOCUS_ONLY === "1") return;
    await runLocalFlow(browserA, observerA);
    await runTabsFlow(browserA, observerA);
    await runDiffFlow(browserA, observerA);
    await runViewerFlow(browserA, observerA);
    await runHeicFlow(browserA, observerA);
    await runQuickEditFlow(browserA, observerA);
    await runGlobalSelectionFlow(browserA, observerA);
    await runGitFlow(browserA, observerA);
    await runGitInitFlow(browserA, observerA);
    await runFedupFlow(browserA, observerA);
    await runHashCheckFlow(browserA, observerA);
    await runHashGenerateFlow(browserA, observerA);
    await runArchiveFlow(browserA, observerA);
    await runSearchActivityFlow(browserA, observerA);
    await runSearchFlow(browserA, observerA);
    await runSearchActionsFlow(browserA, observerA);
    await runDatetimeSearchFlow(browserA, observerA);
    await runSizeSearchFlow(browserA, observerA);

    browserB = await startBrowser("fbro-b", webdriverHosts.b, webdriverPorts.b, appB);
    observerB = new FbroObserver("fbro-b", wsUrls.b);
    await observerB.connect();
    await runRemoteFlow(browserA, observerA, browserB, observerB);
  } catch (error) {
    if (browserA) {
      const focus = await browserA.execute(() => ({
        tag: document.activeElement?.tagName,
        testId: document.activeElement?.getAttribute("data-testid"),
        className: document.activeElement?.className,
        panel: document.activeElement?.closest("[data-panel-id]")?.getAttribute("data-panel-id"),
        modalIds: Array.from(document.querySelectorAll("[data-modal-id]"), (element) => element.getAttribute("data-modal-id")),
      })).catch(() => null);
      console.error("Focus at smoke failure:", focus);
    }
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
