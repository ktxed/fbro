import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { backendPath, hostPath } from "../config.mjs";
import {
  clickCommand,
  clickEntry,
  closeTerminalJobDialog,
  confirmOperation,
  doubleClickEntry,
  pressPanelKey,
  reloadBrowserApp,
} from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runArchiveFlow(browser, observer) {
  const aSrc = backendPath("a", "src");
  const aDst = backendPath("a", "dst");
  const root = path.posix.join(aSrc, "archive-root/nested");
  fs.mkdirSync(hostPath(root), { recursive: true });
  fs.writeFileSync(hostPath(path.posix.join(root, "payload.txt")), "archive payload\n");
  execFileSync("zip", ["-qr", "fixture.zip", "archive-root"], { cwd: hostPath(aSrc) });
  fs.copyFileSync(hostPath(path.posix.join(aSrc, "fixture.zip")), hostPath(path.posix.join(aSrc, "fixture-enter.zip")));
  fs.rmSync(hostPath(path.posix.join(aSrc, "archive-root")), { recursive: true, force: true });

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc, cursorHint: "fixture.zip" });
  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "right", path: aDst });
  await observer.waitForPanelPath("left", aSrc);
  await observer.waitForPanelPath("right", aDst);
  await observer.waitForEntry("left", "fixture.zip", "file");
  await reloadBrowserApp(browser, "fbro-a");
  // Enter on the row remains the archive's primary open behavior.
  await clickEntry(browser, "left", "fixture-enter.zip");
  await pressPanelKey(browser, "left", "Enter");
  await observer.waitForPanelShadow("left");
  await pressPanelKey(browser, "left", "ArrowLeft");
  await observer.waitForPanelPath("left", aSrc);

  // Right enters the inline toolbar on the explicit, first-position Browse action.
  await clickEntry(browser, "left", "fixture.zip");
  await pressPanelKey(browser, "left", "ArrowRight");
  await waitFor(() => browser.execute(() => {
    const actions = [...document.querySelectorAll('[data-entry-name="fixture.zip"] [data-inline-operation]')];
    return actions[0]?.getAttribute("data-inline-operation") === "panel.browse-archive"
      && actions[0] === document.activeElement;
  }), { timeout: 10_000, label: "archive Browse action focused first" });
  await browser.keys("Enter");
  await observer.waitForPanelShadow("left");
  await observer.waitForEntry("left", "archive-root", "directory");
  await waitFor(
    async () => (await browser.$('[data-testid="panel-left"]')).isFocused(),
    {
      timeout: 10_000,
      label: "left panel focus after archive Browse activation",
    },
  );

  // Deliberately avoid pressPanelKey here: it focuses the panel and would mask
  // the regression where the Browse button disappears while retaining focus.
  await browser.keys("ArrowLeft");
  await observer.waitForPanelPath("left", aSrc);

  // Re-enter to retain coverage of archive navigation and extraction.
  await clickEntry(browser, "left", "fixture.zip");
  await pressPanelKey(browser, "left", "Enter");
  await observer.waitForPanelShadow("left");
  await observer.waitForEntry("left", "archive-root", "directory");
  await doubleClickEntry(browser, "left", "archive-root");
  await observer.waitForEntry("left", "nested", "directory");
  await doubleClickEntry(browser, "left", "nested");
  await observer.waitForEntry("left", "payload.txt", "file");
  await clickEntry(browser, "left", "payload.txt");
  await clickCommand(browser, "file.copy");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await closeTerminalJobDialog(browser);

  const extracted = fs.readFileSync(hostPath(path.posix.join(aDst, "payload.txt")), "utf8");
  if (extracted !== "archive payload\n") throw new Error("archive extraction content mismatch");
}
