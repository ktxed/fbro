import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import {
  clickCommand,
  clickContextMenuItem,
  clickEntry,
  confirmOperation,
  doubleClickEntry,
  longRightClickEntry,
  openPanelContextMenu,
  reloadBrowserApp,
} from "../browser.mjs";
import { waitFor } from "../wait.mjs";

const GLOBAL_SOURCE_ID = "global-selections";

export async function runGlobalSelectionFlow(browser, observer) {
  const aSrc = backendPath("a", "src");
  const root = path.posix.join(aSrc, "global-selection-smoke");
  const keyboardDir = path.posix.join(root, "keyboard");
  const appendDir = path.posix.join(root, "append-keyboard");
  const longPressDir = path.posix.join(root, "append-long-press");

  const files = {
    first: path.posix.join(keyboardDir, "first.txt"),
    second: path.posix.join(keyboardDir, "second.txt"),
    third: path.posix.join(appendDir, "third.txt"),
    fourth: path.posix.join(appendDir, "fourth.txt"),
    fifth: path.posix.join(longPressDir, "fifth.txt"),
    sixth: path.posix.join(longPressDir, "sixth.txt"),
  };

  for (const dir of [keyboardDir, appendDir, longPressDir]) {
    fs.mkdirSync(hostPath(dir), { recursive: true });
  }
  for (const [label, file] of Object.entries(files)) {
    fs.writeFileSync(hostPath(file), `${label} payload\n`);
  }

  await navigateLocalPath(browser, observer, "left", keyboardDir, "first.txt");
  await observer.waitForAllEntries("left", ["first.txt", "second.txt"]);
  await selectEntries(browser, observer, "left", ["first.txt", "second.txt"]);

  await openPanelContextMenu(browser, "left");
  await clickContextMenuItem(browser, "To global selection");
  const created = await observer.waitForGlobalSelection("Selection 1", 2);
  if (!created.createdAt || Number.isNaN(Number.parseInt(created.createdAt, 10))) {
    throw new Error(`global selection createdAt should be epoch seconds, got ${created.createdAt}`);
  }
  await observer.waitForSource(GLOBAL_SOURCE_ID);

  observer.sendIntent("NavigateToSource", "left", { sourceId: GLOBAL_SOURCE_ID });
  await waitForPanelSource(observer, "left", GLOBAL_SOURCE_ID);
  await observer.waitForPanelPath("left", "");
  await reloadBrowserApp(browser, observer.name);
  await waitForPanelSource(observer, "left", GLOBAL_SOURCE_ID);
  await observer.waitForPanelPath("left", "");
  const rootEntry = await observer.waitForEntry("left", "Selection 1", "directory");
  if (rootEntry.size !== 2) throw new Error(`Selection 1 root entry should show count 2, got ${rootEntry.size}`);
  if (!rootEntry.mtime) throw new Error("Selection 1 root entry should expose creation date as mtime");

  await doubleClickEntry(browser, "left", "Selection 1");
  await observer.waitForPanelPath("left", "Selection 1");
  await observer.waitForEntry("left", files.first, "file");
  await observer.waitForEntry("left", files.second, "file");

  await clickEntry(browser, "left", files.first);
  await clickCommand(browser, "file.delete");
  await confirmOperation(browser, "file.delete.confirm");
  await observer.waitForGlobalSelection("Selection 1", 1);
  await observer.waitForNoEntry("left", files.first);
  if (!fs.existsSync(hostPath(files.first))) {
    throw new Error("virtual delete removed the real filesystem file");
  }

  await navigateLocalPath(browser, observer, "left", appendDir, "third.txt");
  await observer.waitForAllEntries("left", ["third.txt", "fourth.txt"]);
  await selectEntries(browser, observer, "left", ["third.txt", "fourth.txt"]);
  await openPanelContextMenu(browser, "left");
  await clickContextMenuItem(browser, "To global selection");
  await expectGlobalSelectionSummary(browser, "Add 2 selected entries");
  await confirmGlobalSelectionModal(browser);
  await observer.waitForGlobalSelection("Selection 1", 3);

  await navigateLocalPath(browser, observer, "left", longPressDir, "fifth.txt");
  await observer.waitForAllEntries("left", ["fifth.txt", "sixth.txt"]);
  await selectEntries(browser, observer, "left", ["fifth.txt", "sixth.txt"]);
  await longRightClickEntry(browser, "left", "fifth.txt");
  await clickContextMenuItem(browser, "To global selection");
  await expectGlobalSelectionSummary(browser, "Add 2 selected entries");
  await confirmGlobalSelectionModal(browser);
  await observer.waitForGlobalSelection("Selection 1", 5);

  observer.sendIntent("NavigateToSource", "left", { sourceId: GLOBAL_SOURCE_ID });
  await waitForPanelSource(observer, "left", GLOBAL_SOURCE_ID);
  await observer.waitForPanelPath("left", "");
  await reloadBrowserApp(browser, observer.name);
  await waitForPanelSource(observer, "left", GLOBAL_SOURCE_ID);
  await observer.waitForPanelPath("left", "");
  await observer.waitForEntry("left", "Selection 1", "directory");
  await clickEntry(browser, "left", "Selection 1");
  await clickCommand(browser, "file.delete");
  await confirmOperation(browser, "file.delete.confirm");
  let lastSnapshotRequest = 0;
  await waitFor(() => {
    if (!observer.state.globalSelections.some((selection) => selection.name === "Selection 1")) return true;
    const now = Date.now();
    if (now - lastSnapshotRequest > 500) {
      lastSnapshotRequest = now;
      observer.sendIntent("RequestStateSnapshot");
    }
    return false;
  }, {
    timeout: 10_000,
    label: `${observer.name} Selection 1 removed`,
  });
  await observer.waitForNoEntry("left", "Selection 1");
  for (const file of Object.values(files)) {
    if (!fs.existsSync(hostPath(file))) {
      throw new Error(`global selection root delete removed real filesystem file ${file}`);
    }
  }
}

async function selectEntries(browser, observer, panelId, names) {
  observer.sendIntent("ClearSelection", panelId);
  await waitFor(() => (observer.state.panels[panelId]?.selection ?? []).length === 0, {
    timeout: 10_000,
    label: `${observer.name} clear ${panelId} selection`,
  });
  for (const name of names) {
    await clickEntry(browser, panelId, name);
    await browser.keys(" ");
  }
  await waitFor(async () => {
    const selectedNames = await browser.execute((panelId) => {
      return Array.from(document.querySelectorAll(`[data-testid="panel-${panelId}"] [data-testid="panel-entry"].panel__row--selected`))
        .map((entry) => entry.getAttribute("data-entry-name"));
    }, panelId);
    return names.every((name) => selectedNames.includes(name));
  }, { timeout: 10_000, label: `${observer.name} select ${names.join(", ")}` });
}

async function navigateLocalPath(browser, observer, panelId, targetPath, cursorHint) {
  const sourceId = observer.sourceIdForPath(targetPath);
  if (!sourceId) throw new Error(`no local filesystem source for ${targetPath}`);
  observer.sendIntent("NavigateToSource", panelId, { sourceId });
  await waitForPanelSource(observer, panelId, sourceId);
  observer.sendIntent("NavigatePanelTo", undefined, { panelId, path: targetPath, cursorHint });
  await observer.waitForPanelPath(panelId, targetPath);
  await waitForPanelSource(observer, panelId, sourceId);
  await reloadBrowserApp(browser, observer.name);
  await observer.waitForPanelPath(panelId, targetPath);
  await waitForPanelSource(observer, panelId, sourceId);
}

async function waitForPanelSource(observer, panelId, sourceId) {
  return waitFor(() => {
    const panel = observer.state.panels[panelId];
    return panel?.sourceId === sourceId ? panel : false;
  }, { timeout: 10_000, label: `${observer.name} ${panelId} source ${sourceId}` });
}

async function expectGlobalSelectionSummary(browser, expected) {
  await waitFor(async () => {
    const summary = await browser.$('.modal[aria-label="To global selection"] .modal__summary');
    if (!await summary.isExisting()) return false;
    return (await summary.getText()).trim() === expected;
  }, { timeout: 10_000, label: `global selection modal summary ${expected}` });
}

async function confirmGlobalSelectionModal(browser) {
  const button = await browser.$('.modal[aria-label="To global selection"] .modal__btn--primary');
  await button.waitForClickable({ timeout: 10_000 });
  await button.click();
}
