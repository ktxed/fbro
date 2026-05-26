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

export async function runArchiveFlow(browser, observer) {
  const aSrc = backendPath("a", "src");
  const aDst = backendPath("a", "dst");
  const root = path.posix.join(aSrc, "archive-root/nested");
  fs.mkdirSync(hostPath(root), { recursive: true });
  fs.writeFileSync(hostPath(path.posix.join(root, "payload.txt")), "archive payload\n");
  execFileSync("zip", ["-qr", "fixture.zip", "archive-root"], { cwd: hostPath(aSrc) });
  fs.rmSync(hostPath(path.posix.join(aSrc, "archive-root")), { recursive: true, force: true });

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc, cursorHint: "fixture.zip" });
  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "right", path: aDst });
  await observer.waitForPanelPath("left", aSrc);
  await observer.waitForPanelPath("right", aDst);
  await observer.waitForEntry("left", "fixture.zip", "file");
  await reloadBrowserApp(browser, "fbro-a");
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
