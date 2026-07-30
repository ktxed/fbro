import fs from "node:fs";
import path from "node:path";

import { clickEntry, goToPanelPath } from "../browser.mjs";
import { backendPath, hostPath } from "../config.mjs";
import { waitFor } from "../wait.mjs";

export async function runQuickEditFlow(browser, observer) {
  const directory = backendPath("a", "src");
  const target = path.posix.join(directory, "quick-edit-smoke.txt");
  const hostTarget = hostPath(target);
  fs.writeFileSync(hostTarget, "before\n");

  await goToPanelPath(browser, observer, "left", directory);
  await observer.waitForEntry("left", "quick-edit-smoke.txt", "file");
  await clickEntry(browser, "left", "quick-edit-smoke.txt");
  observer.sendIntent("QuickEditFile", "left");

  let terminalOpened = false;
  await waitFor(() => {
    if (observer.state.panels.left?.terminalSessionId != null) terminalOpened = true;
    const saved = fs.readFileSync(hostTarget, "utf8").includes("quick edit saved");
    return terminalOpened && saved && observer.state.panels.left?.terminalSessionId == null;
  }, { timeout: 10_000, label: "Quick Edit terminal completion" });

  observer.sendIntent("QuickEditFile", "missing-panel");
  observer.sendIntent("NavigateToSource", "left", { sourceId: "journal" });
  await waitFor(() => {
    const panel = observer.state.panels.left;
    if (panel?.sourceId !== "journal" || panel.status !== "ready") return false;
    const names = panel.entries.map((entry) => entry.name);
    return ["quick-edit_v1_intent_", "quick-edit_v1_completed_", "quick-edit_v1_failed_"]
      .every((prefix) => names.some((name) => name.startsWith(prefix)));
  }, { timeout: 10_000, label: "Quick Edit journal events" });

  const sourceId = observer.sourceIdForPath(directory);
  if (!sourceId) throw new Error(`No local source contains ${directory}`);
  observer.sendIntent("NavigateToSource", "left", { sourceId });
  await waitFor(() => observer.state.panels.left?.sourceId === sourceId, {
    timeout: 10_000,
    label: "restore source after Quick Edit journal assertions",
  });
  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: directory });
  await observer.waitForPanelPath("left", directory);
  await observer.waitForPanelReady("left");
}
