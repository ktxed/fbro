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
  pressPanelKey,
  pressWindowKey,
  waitForNewWindow,
} from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runLocalFlow(browser, observer) {
  await verifyColorSchemeSwitch(browser, observer);
  await verifyMarkdownViewerModeSwitch(browser, observer);

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

async function verifyMarkdownViewerModeSwitch(browser, observer) {
  const aSrc = backendPath("a", "src");
  const filename = "viewer-markdown.md";
  const mainHandle = await browser.getWindowHandle();
  fs.writeFileSync(
    hostPath(path.posix.join(aSrc, filename)),
    "# Viewer Markdown\n\nThis file verifies markdown preview mode switching.\n",
  );

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc, cursorHint: filename });
  await observer.waitForPanelPath("left", aSrc);
  await observer.waitForEntry("left", filename, "file");
  await clickEntry(browser, "left", filename);

  const handlesBefore = await browser.getWindowHandles();
  await pressPanelKey(browser, "left", "F3");
  const viewerHandle = await waitForNewWindow(browser, handlesBefore, "markdown viewer window");
  await browser.switchToWindow(viewerHandle);

  try {
    await waitFor(async () => {
      return browser.execute(() => document.querySelector(".viewer__markdown h1")?.textContent === "Viewer Markdown");
    }, { timeout: 15_000, label: "initial markdown render" });

    await pressWindowKey(browser, "3");
    await waitFor(async () => {
      return browser.execute(() => document.querySelector(".viewer__pre")?.textContent?.includes("56 69 65 77"));
    }, { timeout: 10_000, label: "viewer hex mode" });

    await pressWindowKey(browser, "1");
    await waitFor(async () => {
      return browser.execute(() => document.querySelector(".viewer__markdown h1")?.textContent === "Viewer Markdown");
    }, { timeout: 10_000, label: "markdown render after hex text switch" });

    await pressWindowKey(browser, "Escape");
    await waitFor(async () => {
      const handles = await browser.getWindowHandles();
      return !handles.includes(viewerHandle);
    }, { timeout: 10_000, label: "markdown viewer close" });
  } finally {
    const handles = await browser.getWindowHandles();
    if (handles.includes(mainHandle)) {
      await browser.switchToWindow(mainHandle);
    }
  }
}

async function verifyColorSchemeSwitch(browser, observer) {
  await clickCommand(browser, "app.settings");
  const displayTab = await browser.$('[data-testid="settings-tab-display"]');
  await displayTab.waitForClickable({ timeout: 10_000 });
  await displayTab.click();

  const select = await browser.$('[data-testid="color-scheme-select"]');
  await select.waitForDisplayed({ timeout: 10_000 });
  await select.selectByAttribute("value", "daylight");

  await waitFor(() => observer.state.settings?.general?.colorScheme === "daylight", {
    timeout: 10_000,
    label: "daylight color scheme setting",
  });
  await waitFor(async () => {
    return browser.execute(() => {
      const app = document.querySelector(".app");
      const bg = getComputedStyle(app).getPropertyValue("--bg").trim();
      return app?.getAttribute("data-color-scheme") === "daylight" && bg === "#f4f6f8";
    });
  }, { timeout: 10_000, label: "daylight color scheme CSS" });

  const close = await browser.$(".modal--settings .modal__actions .modal__btn");
  await close.waitForClickable({ timeout: 10_000 });
  await close.click();
}
