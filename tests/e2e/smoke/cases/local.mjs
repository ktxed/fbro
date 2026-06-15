import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import {
  clickCommand,
  clickEntry,
  clickSortHeader,
  closeTerminalJobDialog,
  confirmOperation,
  createDirectory,
  createFile,
  doubleClickEntry,
  setTextInputValue,
} from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runLocalFlow(browser, observer) {
  await verifyColorSchemeSwitch(browser, observer);

  const aSrc = backendPath("a", "src");
  const aDst = backendPath("a", "dst");
  await createDirectory(browser, observer, "left", "alpha");
  await doubleClickEntry(browser, "left", "alpha");
  await observer.waitForPanelPath("left", path.posix.join(aSrc, "alpha"));
  await createDirectory(browser, observer, "left", "nested");
  await createFile(browser, observer, "left", "note.txt");
  await createFile(browser, observer, "left", "remove.txt");

  fs.writeFileSync(hostPath(path.posix.join(aSrc, "alpha/note.txt")), "renamed payload\n");
  fs.writeFileSync(hostPath(path.posix.join(aSrc, "alpha/remove.txt")), "delete me\n");

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc, cursorHint: "alpha" });
  await observer.waitForPanelPath("left", aSrc);
  await observer.waitForEntry("left", "alpha", "directory");
  await clickEntry(browser, "left", "alpha");
  await clickCommand(browser, "file.copy");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await observer.waitForEntry("right", "alpha", "directory");
  await closeTerminalJobDialog(browser);

  await clickEntry(browser, "left", "alpha");
  await clickCommand(browser, "file.copy");
  const duplicateWarning = await browser.$('[data-testid="operation-dialog"] .modal__warning');
  await duplicateWarning.waitForDisplayed({ timeout: 10_000 });
  const duplicateCopyButton = await browser.$('[data-command-id="file.copy.confirm"]');
  if (await duplicateCopyButton.isEnabled()) {
    throw new Error("duplicate destination copy button should be disabled");
  }
  await clickCommand(browser, "file.copy.cancel");

  await createFile(browser, observer, "left", "single-copy.txt");
  fs.writeFileSync(hostPath(path.posix.join(aSrc, "single-copy.txt")), "single payload\n");
  await clickEntry(browser, "left", "single-copy.txt");
  await clickCommand(browser, "file.copy");
  await setTextInputValue(browser, '[data-testid="copy-destination-name-input"]', "single-renamed.txt");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await observer.waitForEntry("right", "single-renamed.txt", "file");
  await closeTerminalJobDialog(browser);

  await clickEntry(browser, "left", "alpha");
  await clickCommand(browser, "file.copy");
  await setTextInputValue(browser, '[data-testid="copy-destination-name-input"]', "alpha-copy");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await observer.waitForEntry("right", "alpha-copy", "directory");
  await closeTerminalJobDialog(browser);

  await doubleClickEntry(browser, "right", "alpha");
  await observer.waitForPanelPath("right", path.posix.join(aDst, "alpha"));
  await clickEntry(browser, "right", "note.txt");
  await clickCommand(browser, "file.rename");
  const renameInput = await browser.$('[data-testid="rename-to-input"]');
  await renameInput.waitForDisplayed({ timeout: 10_000 });
  await renameInput.setValue("renamed.txt");
  await clickCommand(browser, "file.rename.confirm");
  await observer.waitForEntry("right", "renamed.txt", "file");

  await clickEntry(browser, "right", "remove.txt");
  await clickCommand(browser, "file.delete");
  await confirmOperation(browser, "file.delete.confirm");
  await observer.waitForJobIdle();
  await observer.waitForNoEntry("right", "remove.txt");
  await closeTerminalJobDialog(browser);

  const copied = fs.readFileSync(hostPath(path.posix.join(aDst, "alpha/renamed.txt")), "utf8");
  if (copied !== "renamed payload\n") throw new Error("renamed copied file content mismatch");
  const renamedSingle = fs.readFileSync(hostPath(path.posix.join(aDst, "single-renamed.txt")), "utf8");
  if (renamedSingle !== "single payload\n") throw new Error("renamed single-file copy content mismatch");
  const renamedFolderFile = fs.readFileSync(hostPath(path.posix.join(aDst, "alpha-copy/note.txt")), "utf8");
  if (renamedFolderFile !== "renamed payload\n") throw new Error("renamed folder copy content mismatch");
  if (fs.existsSync(hostPath(path.posix.join(aDst, "alpha/remove.txt")))) throw new Error("deleted file still exists");

  // ── Sort validation ──────────────────────────────────────────────────────

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc });
  await observer.waitForPanelPath("left", aSrc);
  await createDirectory(browser, observer, "left", "sort-test");
  await doubleClickEntry(browser, "left", "sort-test");
  const sortDir = path.posix.join(aSrc, "sort-test");
  await observer.waitForPanelPath("left", sortDir);

  const hostSortDir = hostPath(sortDir);
  fs.writeFileSync(path.join(hostSortDir, "aardvark.txt"), Buffer.alloc(500));
  fs.writeFileSync(path.join(hostSortDir, "zebra.txt"), Buffer.alloc(10));
  fs.writeFileSync(path.join(hostSortDir, "baby.txt"), Buffer.alloc(100));

  const nowSec = Date.now() / 1000;
  fs.utimesSync(path.join(hostSortDir, "aardvark.txt"), nowSec, nowSec - 3600);
  fs.utimesSync(path.join(hostSortDir, "baby.txt"), nowSec, nowSec - 1800);
  fs.utimesSync(path.join(hostSortDir, "zebra.txt"), nowSec, nowSec);

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc });
  await observer.waitForPanelPath("left", aSrc);
  await doubleClickEntry(browser, "left", "sort-test");
  await observer.waitForPanelPath("left", sortDir);
  const sortFiles = ["aardvark.txt", "zebra.txt", "baby.txt"];
  await observer.waitForAllEntries("left", sortFiles);

  await observer.waitForSort("left", "name", "asc");
  observer.assertFileEntryOrder("left", ["aardvark.txt", "baby.txt", "zebra.txt"]);

  await clickSortHeader(browser, "left", "name");
  await observer.waitForSort("left", "name", "desc");
  await observer.waitForAllEntries("left", sortFiles);
  observer.assertFileEntryOrder("left", ["zebra.txt", "baby.txt", "aardvark.txt"]);

  await clickSortHeader(browser, "left", "size");
  await observer.waitForSort("left", "size", "asc");
  await observer.waitForAllEntries("left", sortFiles);
  observer.assertFileEntryOrder("left", ["zebra.txt", "baby.txt", "aardvark.txt"]);

  await clickSortHeader(browser, "left", "size");
  await observer.waitForSort("left", "size", "desc");
  await observer.waitForAllEntries("left", sortFiles);
  observer.assertFileEntryOrder("left", ["aardvark.txt", "baby.txt", "zebra.txt"]);

  await clickSortHeader(browser, "left", "date");
  await observer.waitForSort("left", "date", "asc");
  await observer.waitForAllEntries("left", sortFiles);
  observer.assertFileEntryOrder("left", ["aardvark.txt", "baby.txt", "zebra.txt"]);

  await clickSortHeader(browser, "left", "date");
  await observer.waitForSort("left", "date", "desc");
  await observer.waitForAllEntries("left", sortFiles);
  observer.assertFileEntryOrder("left", ["zebra.txt", "baby.txt", "aardvark.txt"]);
}

async function verifyColorSchemeSwitch(browser, observer) {
  observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: "daylight" });
  await waitFor(() => observer.state.settings?.general?.colorScheme === "daylight", {
    timeout: 10_000,
    label: "daylight color scheme setting",
  });
  await waitFor(async () => {
    return browser.execute(() => {
      const app = document.querySelector(".app");
      return app?.getAttribute("data-color-scheme") === "daylight";
    });
  }, { timeout: 10_000, label: "daylight color scheme applied" });
}
