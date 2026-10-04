import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import {
  clickCommand,
  clickContextMenuItem,
  clickEntry,
  clickSortHeader,
  closeTerminalJobDialog,
  confirmOperation,
  createDirectory,
  createFile,
  doubleClickEntry,
  expectCheckboxSelected,
  goToPanelPath,
  openSortSelector,
  openPanelContextMenu,
  setTextInputValue,
  pressWindowKey,
} from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus, expectCoveredModal } from "./modal-focus.mjs";
import { verifyThumbnailQueue } from "./thumbnails.mjs";

export async function runLocalFlow(browser, observer) {
  await verifyColorSchemeSwitch(browser, observer);
  await verifyAboutModal(browser);
  await verifyEjectPicker(browser);
  await verifyThumbnailQueue(browser, observer);

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

  await verifyLocalSymlinkNavigation(browser, observer, aSrc);

  await goToPanelPath(browser, observer, "left", aSrc);
  await observer.waitForEntry("left", "alpha", "directory");
  await clickEntry(browser, "left", "alpha");
  await verifyBroHelp(browser);
  await clickCommand(browser, "file.copy");
  await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "copy primary action focus");
  await expectCheckboxSelected(
    browser,
    '[data-testid="copy-save-digest-file-input"]',
    false,
    "local copy digest checkbox default",
  );
  let reachedCopyActionByTab = false;
  for (let i = 0; i < 12 && !reachedCopyActionByTab; i += 1) {
    await browser.keys("Tab");
    reachedCopyActionByTab = await browser.execute(() => (
      document.activeElement?.getAttribute("data-command-id") === "file.copy.confirm"
    ));
  }
  if (!reachedCopyActionByTab) throw new Error("Copy action was not reachable by Tab");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await observer.waitForEntry("right", "alpha", "directory");
  await closeTerminalJobDialog(browser);

  await clickEntry(browser, "left", "alpha");
  await clickCommand(browser, "file.copy");
  await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "existing destination copy focus");
  const duplicateCopyButton = await browser.$('[data-command-id="file.copy.confirm"]');
  if (!await duplicateCopyButton.isEnabled()) {
    throw new Error("existing destination must allow Copy with Ask");
  }
  await browser.execute(() => document.querySelector('[data-command-id="file.copy.cancel"]')?.focus());
  await browser.keys("Enter");
  await (await browser.$('[data-testid="operation-dialog"]')).waitForDisplayed({ timeout: 10_000, reverse: true });

  await createFile(browser, observer, "left", "single-copy.txt");
  fs.writeFileSync(hostPath(path.posix.join(aSrc, "single-copy.txt")), "single payload\n");
  await clickEntry(browser, "left", "single-copy.txt");
  await clickCommand(browser, "file.copy");
  await browser.keys("n");
  await expectModalFocus(browser, '[data-testid="copy-destination-name-input"]', "copy name accelerator focus");
  const nameSelected = await browser.execute(() => {
    const input = document.querySelector('[data-testid="copy-destination-name-input"]');
    return input?.selectionStart === 0 && input?.selectionEnd === input?.value.length;
  });
  if (!nameSelected) throw new Error("Copy name accelerator did not select the destination name");
  await browser.keys("n");
  if (await (await browser.$('[data-testid="copy-destination-name-input"]')).getValue() !== "n") {
    throw new Error("Copy name accelerator intercepted typing in the destination name");
  }
  await setTextInputValue(browser, '[data-testid="copy-destination-name-input"]', "single-renamed.txt");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await observer.waitForEntry("right", "single-renamed.txt", "file");
  await closeTerminalJobDialog(browser);

  // Copy to an existing file: Ask must defer replacement until the decision.
  await clickEntry(browser, "left", "single-copy.txt");
  await browser.keys("F5");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await observer.waitForEntry("right", "single-copy.txt", "file");
  await closeTerminalJobDialog(browser);
  const existingCopyPath = hostPath(path.posix.join(aDst, "single-copy.txt"));
  fs.writeFileSync(existingCopyPath, "old destination payload\n");
  await clickEntry(browser, "left", "single-copy.txt");
  await browser.keys("F5");
  await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "existing file copy focus");
  await browser.keys("Enter");
  await expectModalFocus(browser, '[data-command-id="job.decision.overwrite"]', "overwrite decision focus");
  if (fs.readFileSync(existingCopyPath, "utf8") !== "old destination payload\n") {
    throw new Error("Ask replaced the existing file before the overwrite decision");
  }
  await browser.keys("Enter");
  await observer.waitForJobIdle();
  await closeTerminalJobDialog(browser);
  if (fs.readFileSync(existingCopyPath, "utf8") !== "single payload\n") {
    throw new Error("Ask overwrite decision did not replace the existing file");
  }
  await waitFor(() => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"), {
    timeout: 10_000, label: "copy conflict restores originating panel focus",
  });

  // An edited destination name must also allow the Overwrite policy.
  const renamedCopyPath = hostPath(path.posix.join(aDst, "single-renamed.txt"));
  fs.writeFileSync(renamedCopyPath, "old renamed destination\n");
  await clickEntry(browser, "left", "single-copy.txt");
  await browser.keys("F5");
  await setTextInputValue(browser, '[data-testid="copy-destination-name-input"]', "single-renamed.txt");
  await (await browser.$('[data-testid="file-conflict-policy"]')).selectByAttribute("value", "overwrite");
  await browser.execute(() => document.querySelector('[data-command-id="file.copy.confirm"]')?.focus());
  await browser.keys("Enter");
  await observer.waitForJobIdle();
  await closeTerminalJobDialog(browser);
  if (fs.readFileSync(renamedCopyPath, "utf8") !== "single payload\n") {
    throw new Error("Overwrite did not replace the existing edited destination");
  }

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
  await expectModalFocus(browser, '[data-testid="rename-to-input"]', "rename input focus");
  await browser.execute(() => document.querySelector('[data-command-id="file.rename.cancel"]')?.focus());
  await browser.keys("Enter");
  await (await browser.$('[data-testid="rename-dialog"]')).waitForDisplayed({ timeout: 10_000, reverse: true });
  await observer.waitForEntry("right", "note.txt", "file");
  await clickCommand(browser, "file.rename");
  const renameInput = await browser.$('[data-testid="rename-to-input"]');
  await renameInput.waitForDisplayed({ timeout: 10_000 });
  await renameInput.setValue("renamed.txt");
  await clickCommand(browser, "file.rename.confirm");
  await observer.waitForEntry("right", "renamed.txt", "file");

  await clickEntry(browser, "right", "remove.txt");
  await browser.keys("F8");
  await expectModalFocus(browser, '[data-command-id="file.delete.confirm"]', "delete primary action focus");
  await browser.keys("Tab");
  await expectModalFocus(browser, '[data-command-id="file.delete.background"]', "delete background focus");
  await browser.keys("Tab");
  await expectModalFocus(browser, '[data-command-id="file.delete.cancel"]', "delete cancel focus");
  await browser.keys("Enter");
  await (await browser.$('[data-testid="operation-dialog"]')).waitForDisplayed({ timeout: 10_000, reverse: true });
  await observer.waitForEntry("right", "remove.txt", "file");
  await waitFor(() => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-right"), {
    timeout: 10_000, label: "delete cancel restores originating panel focus",
  });
  await browser.keys("F8");
  await expectModalFocus(browser, '[data-command-id="file.delete.confirm"]', "reopened delete primary action focus");
  await browser.keys("Enter");
  await observer.waitForJobIdle();
  await observer.waitForNoEntry("right", "remove.txt");
  await expectModalFocus(browser, '[data-command-id="job.close"]', "completed delete close focus");
  await closeTerminalJobDialog(browser);

  const copied = fs.readFileSync(hostPath(path.posix.join(aDst, "alpha/renamed.txt")), "utf8");
  if (copied !== "renamed payload\n") throw new Error("renamed copied file content mismatch");
  const renamedSingle = fs.readFileSync(hostPath(path.posix.join(aDst, "single-renamed.txt")), "utf8");
  if (renamedSingle !== "single payload\n") throw new Error("renamed single-file copy content mismatch");
  const renamedFolderFile = fs.readFileSync(hostPath(path.posix.join(aDst, "alpha-copy/note.txt")), "utf8");
  if (renamedFolderFile !== "renamed payload\n") throw new Error("renamed folder copy content mismatch");
  if (fs.existsSync(hostPath(path.posix.join(aDst, "alpha/remove.txt")))) throw new Error("deleted file still exists");

  // ── Sort validation ──────────────────────────────────────────────────────

  await goToPanelPath(browser, observer, "left", aSrc);
  await createDirectory(browser, observer, "left", "sort-test");
  const sortDir = path.posix.join(aSrc, "sort-test");
  await clickEntry(browser, "left", "sort-test");
  const folderInlineOperations = await browser.$$('[data-testid="panel-left"] [data-entry-name="sort-test"] [data-inline-operation]');
  if (folderInlineOperations.length !== 0) throw new Error("singleton directory unexpectedly rendered inline operations");
  await browser.keys("ArrowRight");
  await observer.waitForPanelPath("left", sortDir);

  const hostSortDir = hostPath(sortDir);
  fs.writeFileSync(path.join(hostSortDir, "aardvark.txt"), Buffer.alloc(500));
  fs.writeFileSync(path.join(hostSortDir, "zebra.txt"), Buffer.alloc(10));
  fs.writeFileSync(path.join(hostSortDir, "baby.txt"), Buffer.alloc(100));
  fs.mkdirSync(path.join(hostSortDir, "selected-folder"));

  const nowSec = Date.now() / 1000;
  fs.utimesSync(path.join(hostSortDir, "aardvark.txt"), nowSec, nowSec - 3600);
  fs.utimesSync(path.join(hostSortDir, "baby.txt"), nowSec, nowSec - 1800);
  fs.utimesSync(path.join(hostSortDir, "zebra.txt"), nowSec, nowSec);

  await goToPanelPath(browser, observer, "left", aSrc);
  await doubleClickEntry(browser, "left", "sort-test");
  await observer.waitForPanelPath("left", sortDir);
  const sortFiles = ["aardvark.txt", "zebra.txt", "baby.txt"];
  await observer.waitForAllEntries("left", sortFiles);
  await observer.waitForEntry("left", "selected-folder", "directory");

  await observer.waitForSort("left", "name", "asc");
  observer.assertFileEntryOrder("left", ["aardvark.txt", "baby.txt", "zebra.txt"]);
  await verifySelectedFolderInlineNavigation(browser, observer, sortDir, "aardvark.txt");
  await verifyContextualSelectionOperations(browser, observer, sortFiles);

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

  await verifyLiveDateSortCursorAnchor(browser, observer, hostSortDir, nowSec);

  await openSortSelector(browser, "left");
  await browser.keys(["ArrowLeft"]);
  await browser.keys(["ArrowDown"]);
  await browser.keys(["ArrowDown"]);
  await browser.keys(["Enter"]);
  await observer.waitForSort("left", "size", "desc");
  await observer.waitForAllEntries("left", sortFiles);
  observer.assertFileEntryOrder("left", ["aardvark.txt", "baby.txt", "zebra.txt"]);

  await openSortSelector(browser, "left");
  await browser.keys(["ArrowRight"]);
  await browser.keys(["ArrowUp"]);
  await browser.keys(["ArrowUp"]);
  await browser.keys(["Escape"]);
  await observer.waitForSort("left", "size", "desc");
  observer.assertFileEntryOrder("left", ["aardvark.txt", "baby.txt", "zebra.txt"]);

  const leftPanel = await browser.$('[data-testid="panel-left"]');
  await leftPanel.click();
  await browser.keys(["f"]);
  const filterInput = await browser.$('[data-testid="panel-left"] .panel__filter-input');
  await filterInput.waitForDisplayed({ timeout: 10_000 });
  await filterInput.click();

  await openSortSelector(browser, "left", { preserveFocus: true });
  await browser.keys(["ArrowRight"]);
  await browser.keys(["ArrowUp"]);
  await browser.keys(["Enter"]);
  await observer.waitForSort("left", "date", "asc");
  await waitFor(async () => await browser.execute(() => document.activeElement?.classList.contains("panel__filter-input") === true), {
    timeout: 5_000,
    label: "filter input focus after closing sort selector",
  });
  await browser.keys(["Escape"]);

  await verifyFilterEscapeRestoresCompleteListing(browser, observer, aSrc);

  // Per-location sorting is opt-in. While disabled, navigation carries the
  // panel's current sort and another panel at the same path remains independent.
  if (observer.state.settings?.general?.rememberSortPerLocation !== false) {
    throw new Error("per-location sorting should default to disabled");
  }
  await goToPanelPath(browser, observer, "left", aSrc);
  await observer.waitForSort("left", "date", "asc");
  await goToPanelPath(browser, observer, "right", sortDir);
  await observer.waitForSort("right", "name", "asc");
  await goToPanelPath(browser, observer, "left", sortDir);
  await observer.waitForSort("left", "date", "asc");
  await clickSortHeader(browser, "left", "date");
  await observer.waitForSort("left", "date", "desc");
  await observer.waitForSort("right", "name", "asc");

  observer.sendIntent("SetGeneralOption", undefined, { key: "rememberSortPerLocation", value: true });
  await waitFor(() => observer.state.settings?.general?.rememberSortPerLocation === true, {
    timeout: 10_000,
    label: "enable per-location sorting",
  });

  // Once enabled, the next sort is cached and synchronized across panels at
  // the same local location. Unseen locations use name ascending.
  await clickSortHeader(browser, "left", "date");
  await observer.waitForSort("left", "date", "asc");
  await observer.waitForSort("right", "date", "asc");
  await waitFor(() => observer.state.settings?.general?.locationSettingsCount === 1, {
    timeout: 10_000,
    label: "remembered location count",
  });
  await goToPanelPath(browser, observer, "left", aSrc);
  await observer.waitForSort("left", "name", "asc");
  await goToPanelPath(browser, observer, "left", sortDir);
  await observer.waitForSort("left", "date", "asc");

  await verifyLocationSortReset(browser, observer);
  await verifyInlinePreviewNavigation(browser, observer, aSrc, sortDir);
  await verifyBookmarksKeyboardNavigation(browser, observer, aSrc, sortDir);
  await goToPanelPath(browser, observer, "right", path.posix.join(aDst, "alpha"));

  await verifyWindowFocusRestoration(browser, observer);
  await verifyGridLeftAtFirstItemNavigatesUp(browser, observer, aSrc);
}

async function verifyLocalSymlinkNavigation(browser, observer, aSrc) {
  const linksDir = path.posix.join(aSrc, "symlink-test");
  const hostLinksDir = hostPath(linksDir);
  fs.mkdirSync(hostLinksDir);
  fs.symlinkSync("../alpha", path.join(hostLinksDir, "folder-link"), "dir");
  fs.symlinkSync("../alpha/note.txt", path.join(hostLinksDir, "file-link.txt"), "file");
  fs.symlinkSync("missing.txt", path.join(hostLinksDir, "broken-link"), "file");

  await goToPanelPath(browser, observer, "left", linksDir);
  const folder = await observer.waitForEntry("left", "folder-link", "symlink");
  const file = await observer.waitForEntry("left", "file-link.txt", "symlink");
  const broken = await observer.waitForEntry("left", "broken-link", "symlink");
  if (folder.targetKind !== "directory" || folder.pathHint !== path.posix.join(aSrc, "alpha")) {
    throw new Error(`directory link target missing: ${JSON.stringify(folder)}`);
  }
  if (file.targetKind !== "file" || file.pathHint !== path.posix.join(aSrc, "alpha/note.txt")) {
    throw new Error(`file link target missing: ${JSON.stringify(file)}`);
  }
  if (broken.targetKind) throw new Error("broken link unexpectedly has a target type");

  await clickEntry(browser, "left", "folder-link");
  await waitFor(async () => {
    const hint = await browser.$('[data-testid="panel-left"] [data-entry-name="folder-link"] [data-testid="path-hint"]');
    return (await hint.getText()).includes(folder.pathHint);
  }, { timeout: 10_000, label: "directory link target hint" });
  await browser.keys("ArrowRight");
  await observer.waitForPanelPath("left", path.posix.join(aSrc, "alpha"));
  // Children still go Up normally; the target boundary returns through the link.
  await clickEntry(browser, "left", "nested");
  await browser.keys("ArrowRight");
  await observer.waitForPanelPath("left", path.posix.join(aSrc, "alpha/nested"));
  await browser.keys("ArrowLeft");
  await observer.waitForPanelPath("left", path.posix.join(aSrc, "alpha"));
  await browser.keys("ArrowLeft");
  await observer.waitForPanelPath("left", linksDir);
  const expectLinkCursor = () => waitFor(() => {
    const panel = observer.state.panels.left;
    return panel?.entries[panel.cursor]?.name === "folder-link";
  }, { timeout: 10_000, label: "return selects directory link" });
  await expectLinkCursor();

  await browser.keys("ArrowRight");
  await observer.waitForPanelPath("left", path.posix.join(aSrc, "alpha"));
  await browser.keys("[");
  await observer.waitForPanelPath("left", linksDir);
  await expectLinkCursor();
  await browser.keys("]");
  await observer.waitForPanelPath("left", path.posix.join(aSrc, "alpha"));
  await browser.keys("ArrowLeft");
  await observer.waitForPanelPath("left", linksDir);
  await expectLinkCursor();

  await goToPanelPath(browser, observer, "left", linksDir);
  const fileIndex = observer.state.panels.left?.entries.findIndex((entry) => entry.name === "file-link.txt") ?? -1;
  if (fileIndex < 1) throw new Error("file link has no previous viewer entry");
  observer.sendIntent("SetCursor", "left", { index: fileIndex - 1 });
  await waitFor(() => observer.state.panels.left?.cursor === fileIndex - 1, {
    timeout: 10_000, label: "viewer cursor before file link",
  });
  const messageCount = observer.messages.length;
  observer.sendIntent("ViewerNavigate", undefined, { panelId: "left", direction: "next" });
  await waitFor(() => observer.messages.slice(messageCount).find((message) =>
    message.type === "ViewerFileChanged" && message.path === path.posix.join(aSrc, "alpha/note.txt")), {
    timeout: 10_000, label: "viewer follows file link",
  });
  await clickEntry(browser, "left", "broken-link");
  await browser.keys("Enter");
  await waitFor(async () => {
    const hint = await browser.$(".transient-hint");
    return (await hint.isDisplayed()) && (await hint.getText()).includes("Cannot follow symbolic link");
  }, { timeout: 10_000, label: "broken link reports navigation error" });
  if (observer.state.panels.left?.path !== linksDir) throw new Error("broken link changed panel path");
}

async function verifyBookmarksKeyboardNavigation(browser, observer, aSrc, restorePath) {
  const bookmarkNames = Array.from({ length: 18 }, (_, index) =>
    `bookmark-target-${String(index).padStart(2, "0")}`
  );
  const bookmarkPaths = bookmarkNames.map((name) => path.posix.join(aSrc, name));
  for (const bookmarkPath of bookmarkPaths) {
    fs.mkdirSync(hostPath(bookmarkPath), { recursive: true });
    observer.sendIntent("AddBookmark", undefined, {
      name: path.posix.basename(bookmarkPath),
      path: bookmarkPath,
    });
  }
  await waitFor(() => bookmarkPaths.every((bookmarkPath) =>
    observer.state.bookmarks?.some((bookmark) => bookmark.path === bookmarkPath)), {
    timeout: 10_000,
    label: "bookmark keyboard fixtures",
  });

  await goToPanelPath(browser, observer, "left", aSrc);
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  const cursorBefore = observer.state.panels.left.cursor;
  await browser.performActions([{
    type: "key",
    id: "keyboard",
    actions: [
      { type: "keyDown", value: "\uE009" },
      { type: "keyDown", value: "d" },
      { type: "keyUp", value: "d" },
      { type: "keyUp", value: "\uE009" },
    ],
  }]);
  await browser.releaseActions();

  const popover = await browser.$('[data-testid="bookmarks-popover"]');
  await popover.waitForDisplayed({ timeout: 10_000 });

  const selectedBookmarkPath = () => browser.execute(() =>
    document.querySelector('[data-testid="bookmark-option"][data-selected="true"] .bookmarks-popover__item-path')?.textContent ?? null
  );
  if (await selectedBookmarkPath() !== bookmarkPaths[0]) {
    throw new Error("bookmark list did not initially select its first entry");
  }

  await browser.keys("ArrowDown");
  await waitFor(async () => await selectedBookmarkPath() === bookmarkPaths[1], {
    timeout: 10_000,
    label: "bookmark ArrowDown selection",
  });
  if (observer.state.panels.left.cursor !== cursorBefore) {
    throw new Error("bookmark ArrowDown leaked through to the panel cursor");
  }

  const heights = await browser.execute(() => {
    const panel = document.querySelector('[data-testid="panel-left"]');
    const list = document.querySelector('[data-testid="bookmarks-popover"] [role="listbox"]');
    return {
      panel: panel?.getBoundingClientRect().height ?? 0,
      list: list?.getBoundingClientRect().height ?? 0,
    };
  });
  if (heights.list <= 0 || heights.list > heights.panel / 2 + 1) {
    throw new Error(`bookmark list height ${heights.list} exceeds half its panel height ${heights.panel}`);
  }

  await browser.keys("Enter");
  await observer.waitForPanelPath("left", bookmarkPaths[1]);
  await popover.waitForDisplayed({ timeout: 10_000, reverse: true });

  await goToPanelPath(browser, observer, "left", aSrc);
  for (const bookmarkPath of bookmarkPaths) {
    observer.sendIntent("RemoveBookmark", undefined, { path: bookmarkPath });
    fs.rmSync(hostPath(bookmarkPath), { recursive: true });
  }
  await waitFor(() => bookmarkPaths.every((bookmarkPath) =>
    !observer.state.bookmarks?.some((bookmark) => bookmark.path === bookmarkPath)), {
    timeout: 10_000,
    label: "bookmark keyboard fixture cleanup",
  });
  await goToPanelPath(browser, observer, "left", restorePath);
}

async function verifyInlinePreviewNavigation(browser, observer, aSrc, restorePath) {
  const fixturePath = path.posix.join(aSrc, "inline-preview-navigation");
  const hostFixturePath = hostPath(fixturePath);
  fs.mkdirSync(hostFixturePath, { recursive: true });
  const imageNames = ["01-preview.png", "02-preview.png"];
  const encodedImages = await browser.execute(() => ["#d34a4a", "#4a72d3"].map((color) => {
    const canvas = document.createElement("canvas");
    canvas.width = 96;
    canvas.height = 64;
    const context = canvas.getContext("2d");
    context.fillStyle = color;
    context.fillRect(0, 0, canvas.width, canvas.height);
    return canvas.toDataURL("image/png").split(",")[1];
  }));
  imageNames.forEach((name, index) => {
    fs.writeFileSync(path.join(hostFixturePath, name), Buffer.from(encodedImages[index], "base64"));
  });
  fs.writeFileSync(path.join(hostFixturePath, "03-stop.txt"), "preview sequence boundary\n");

  await goToPanelPath(browser, observer, "left", fixturePath);
  await observer.waitForAllEntries("left", [...imageNames, "03-stop.txt"]);
  observer.sendIntent("SetSort", "left", { by: "name", order: "asc" });
  await observer.waitForSort("left", "name", "asc");
  // Observer scan replies belong to that connection. Refresh through the app
  // so its thumbnail URLs use the backend's current listing epoch too.
  await goToPanelPath(browser, observer, "left", fixturePath);
  await clickEntry(browser, "left", imageNames[0]);
  await browser.keys("ArrowRight");
  const firstPreview = await browser.$(`[data-entry-name="${imageNames[0]}"] [data-testid="file-inline-preview"]`);
  await waitFor(() => firstPreview.isFocused(), {
    timeout: 10_000,
    label: "inline image preview keyboard focus",
  });
  await browser.$(".panel__file-preview-tooltip").waitForDisplayed({ timeout: 10_000 });
  await browser.$(`.panel__file-preview-tooltip img[alt="${imageNames[0]}"]`).waitForDisplayed({ timeout: 10_000 });

  await browser.keys("ArrowDown");
  await waitFor(() => browser.execute(() =>
    document.querySelector('[data-testid="panel-left"] .panel__row--cursor')?.getAttribute("data-entry-name") === "02-preview.png"), {
    timeout: 10_000,
    label: "cursor moved to second preview image",
  });
  await browser.$(".panel__file-preview-tooltip").waitForDisplayed({ timeout: 10_000 });
  await browser.$(`.panel__file-preview-tooltip img[alt="${imageNames[1]}"]`).waitForDisplayed({ timeout: 10_000 });
  const secondPreviewDescribed = await browser.$(`[data-entry-name="${imageNames[1]}"] [data-testid="file-inline-preview"]`).getAttribute("aria-describedby");
  if (secondPreviewDescribed !== "file-inline-preview-left") {
    throw new Error(`inline preview did not re-anchor to the second image: ${secondPreviewDescribed}`);
  }

  await browser.keys("ArrowDown");
  await waitFor(() => browser.execute(() =>
    document.querySelector('[data-testid="panel-left"] .panel__row--cursor')?.getAttribute("data-entry-name") === "03-stop.txt"), {
    timeout: 10_000,
    label: "cursor moved to inline preview sequence boundary",
  });
  await browser.$(".panel__file-preview-tooltip").waitForDisplayed({ timeout: 10_000, reverse: true });

  await goToPanelPath(browser, observer, "left", restorePath);
  fs.rmSync(hostFixturePath, { recursive: true });
}

async function verifyFilterEscapeRestoresCompleteListing(browser, observer, aSrc) {
  const fixtureName = "filter-escape-race";
  const fixturePath = path.posix.join(aSrc, fixtureName);
  const hostFixturePath = hostPath(fixturePath);
  fs.mkdirSync(hostFixturePath, { recursive: true });
  const expectedNames = Array.from({ length: 84 }, (_, index) =>
    `entry-${String(index).padStart(3, "0")}.${index < 4 ? "txt" : "dat"}`
  );
  for (const name of expectedNames) fs.writeFileSync(path.join(hostFixturePath, name), name);

  await goToPanelPath(browser, observer, "left", fixturePath);
  await observer.waitForEntry("left", expectedNames.at(-1), "file");
  await waitFor(async () => browser.execute((names) => {
    const rendered = new Set([...document.querySelectorAll('[data-testid="panel-left"] [data-entry-name]')]
      .map((row) => row.getAttribute("data-entry-name")));
    return names.every((name) => rendered.has(name));
  }, expectedNames), { timeout: 10_000, label: "complete filter race fixture" });

  const panel = await browser.$('[data-testid="panel-left"]');
  await panel.click();
  await browser.keys(["f"]);
  const input = await browser.$('[data-testid="panel-left"] .panel__filter-input');
  await input.waitForDisplayed({ timeout: 10_000 });
  await input.setValue("txt");
  await waitFor(async () => browser.execute(() => {
    const names = [...document.querySelectorAll('[data-testid="panel-left"] [data-entry-name]')]
      .map((row) => row.getAttribute("data-entry-name"));
    return names.length === 4 && names.every((name) => name?.endsWith(".txt"));
  }), { timeout: 10_000, label: "filtered race fixture" });

  await browser.keys(["Escape"]);
  await waitFor(async () => browser.execute((names) => {
    const rendered = new Set([...document.querySelectorAll('[data-testid="panel-left"] [data-entry-name]')]
      .map((row) => row.getAttribute("data-entry-name")));
    return names.every((name) => rendered.has(name));
  }, expectedNames), { timeout: 10_000, label: "complete listing after clearing filter" });

  await goToPanelPath(browser, observer, "left", aSrc);
  fs.rmSync(hostFixturePath, { recursive: true });
}

async function verifyLocationSortReset(browser, observer) {
  await pressWindowKey(browser, ",", { meta: true });
  await pressWindowKey(browser, ",", { ctrl: true });
  const settings = await browser.$('[data-modal-id="settings"]');
  await settings.waitForDisplayed({ timeout: 10_000 });

  await expectModalFocus(browser, '[data-testid="settings-tab-general"]', "settings active tab focus");
  await browser.keys("Tab");
  const skippedInactiveTabs = await browser.execute(() => {
    const active = document.activeElement;
    return document.querySelector('[data-modal-id="settings"]')?.contains(active)
      && !active?.matches('[role="tab"][aria-selected="false"]');
  });
  if (!skippedInactiveTabs) throw new Error("Settings Tab did not skip inactive sidebar tabs");
  await browser.keys(["Shift", "Tab"]);
  await expectModalFocus(browser, '[data-testid="settings-tab-general"]', "settings reverse Tab returns to active tab");
  await browser.keys("ArrowDown");
  await browser.keys("ArrowDown");
  await waitFor(() => browser.execute(() => {
    const tab = document.querySelector('[data-testid="settings-tab-bros"]');
    const items = [...document.querySelectorAll('[data-testid="settings-bro-item"]')];
    return tab === document.activeElement
      && tab?.getAttribute("aria-selected") === "true"
      && items.length > 0
      && items.every((item) => item.querySelector("strong")?.textContent?.trim()
        && item.querySelector("span")?.textContent?.trim());
  }), { timeout: 10_000, label: "settings bros catalog" });
  await browser.keys("ArrowDown");
  await waitFor(() => browser.execute(() => {
    const tab = document.querySelector('[data-testid="settings-tab-sharing"]');
    return tab === document.activeElement && tab?.getAttribute("aria-selected") === "true";
  }), { timeout: 10_000, label: "settings roving tab focus" });
  await browser.keys("ArrowUp");
  await browser.keys("ArrowUp");
  await browser.keys("ArrowUp");
  await waitFor(() => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "settings-tab-general"), {
    timeout: 10_000,
    label: "settings roving tab focus restored",
  });

  const toggle = await browser.$('[data-testid="remember-sort-per-location"]');
  if (!(await toggle.isSelected())) throw new Error("location sort toggle is not enabled in settings");
  await (await browser.$('[data-testid="remember-sort-per-location"]')).click();
  await waitFor(() => observer.state.settings?.general?.rememberSortPerLocation === false, {
    timeout: 10_000,
    label: "disable per-location sorting from settings",
  });
  const countHint = await browser.$('[data-testid="location-settings-count"]');
  await waitFor(async () => (await countHint.getText()).includes("retained for up to 30 days"), {
    timeout: 10_000,
    label: "location sort retention hint",
  });
  await (await browser.$('[data-testid="remember-sort-per-location"]')).click();
  await waitFor(() => observer.state.settings?.general?.rememberSortPerLocation === true, {
    timeout: 10_000,
    label: "re-enable per-location sorting from settings",
  });
  const reset = await browser.$('[data-testid="reset-location-sorts"]');
  if (!(await reset.isEnabled())) throw new Error("location sort reset should be enabled for cached locations");

  await reset.click();
  const confirmation = await browser.$('[data-testid="reset-location-sorts-modal"]');
  await confirmation.waitForDisplayed({ timeout: 10_000 });
  await expectCoveredModal(browser, "settings");
  await expectModalFocus(browser, '[data-modal-id="reset-location-sorts"] .modal__btn', "reset defaults to cancel");
  await browser.keys("Escape");
  await confirmation.waitForDisplayed({ timeout: 10_000, reverse: true });
  await expectModalFocus(browser, '[data-testid="reset-location-sorts"]', "nested reset restores trigger focus");
  if (observer.state.settings?.general?.locationSettingsCount !== 1) {
    throw new Error("cancelling location sort reset changed the cached count");
  }

  await (await browser.$('[data-testid="reset-location-sorts"]')).click();
  await (await browser.$('[data-testid="reset-location-sorts-modal"]')).waitForDisplayed({ timeout: 10_000 });
  await (await browser.$('[data-testid="confirm-reset-location-sorts"]')).click();
  await waitFor(() => observer.state.settings?.general?.locationSettingsCount === 0, {
    timeout: 10_000,
    label: "reset remembered location sorts",
  });
  await browser.keys("Escape");
  await settings.waitForDisplayed({ timeout: 10_000, reverse: true });
}

async function verifyBroHelp(browser) {
  // The palette closes in the same update that opens Pass to Bro; the list must
  // still get initial focus rather than the first control (the help button).
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await pressWindowKey(browser, "p", { shift: true, meta: true });
  await pressWindowKey(browser, "p", { shift: true, ctrl: true });
  await (await browser.$(".command-palette__input")).waitForDisplayed({ timeout: 10_000 });
  await setTextInputValue(browser, ".command-palette__input", "Pass to Bro");
  await browser.keys("Enter");
  await (await browser.$('[data-testid="bro-option"]')).waitForDisplayed({ timeout: 10_000 });
  await waitFor(() => browser.execute(() => (
    document.activeElement?.matches('[data-testid="bro-dialog"] [role="listbox"]') ?? false
  )), { timeout: 10_000, label: "bro list initial focus after command palette" });
  // Tab and Shift+Tab move the list selection like the arrow keys and keep focus in the list.
  const broListState = () => browser.execute(() => ({
    selected: Array.from(document.querySelectorAll('[data-testid="bro-option"]'))
      .findIndex((option) => option.getAttribute("data-selected") === "true"),
    listFocused: document.activeElement?.matches('[data-testid="bro-dialog"] [role="listbox"]') ?? false,
  }));
  await browser.keys("Tab");
  await waitFor(async () => {
    const { selected, listFocused } = await broListState();
    return selected === 1 && listFocused;
  }, { timeout: 10_000, label: "Tab moves bro selection down" });
  await browser.keys(["Shift", "Tab"]);
  await waitFor(async () => {
    const { selected, listFocused } = await broListState();
    return selected === 0 && listFocused;
  }, { timeout: 10_000, label: "Shift+Tab moves bro selection up" });
  await browser.keys("Escape");
  await (await browser.$('[data-testid="bro-dialog"]')).waitForDisplayed({ timeout: 10_000, reverse: true });

  await openPanelContextMenu(browser, "left");
  await clickContextMenuItem(browser, "Pass to Bro…");
  const broDialog = await browser.$('[data-testid="bro-dialog"]');
  await broDialog.waitForDisplayed({ timeout: 10_000 });

  const zipBro = await browser.$('[data-testid="bro-option"][data-bro-id="native:zip"]');
  await zipBro.waitForDisplayed({ timeout: 10_000 });
  await zipBro.click();
  const compressionLevel = await browser.$('[data-testid="bro-dialog"] input[type="number"]');
  await compressionLevel.waitForDisplayed({ timeout: 10_000 });
  await compressionLevel.setValue("4");

  await (await browser.$('[data-testid="bro-help-button"]')).click();
  const settings = await browser.$('[data-modal-id="settings"]');
  await settings.waitForDisplayed({ timeout: 10_000 });
  await expectCoveredModal(browser, "bro");
  await expectModalFocus(browser, '[data-testid="settings-tab-bros"]', "bro help active settings tab focus");
  await waitFor(() => browser.execute(() => (
    document.querySelector('[data-testid="settings-tab-bros"]')?.getAttribute("aria-selected") === "true"
    && document.querySelectorAll('[data-testid="settings-bro-item"]').length > 0
  )), { timeout: 10_000, label: "bro help catalog" });

  await (await browser.$('[data-modal-id="settings"] .modal__actions .modal__btn')).click();
  await settings.waitForDisplayed({ timeout: 10_000, reverse: true });
  await broDialog.waitForDisplayed({ timeout: 10_000 });
  await waitFor(() => browser.execute(() => (
    document.activeElement?.getAttribute("data-testid") === "bro-help-button"
  )), { timeout: 10_000, label: "bro picker focus restoration" });
  if (await compressionLevel.getValue() !== "4") {
    throw new Error("closing bro help reset the bro form values");
  }

  await browser.keys("Escape");
  await broDialog.waitForDisplayed({ timeout: 10_000, reverse: true });

  // The backend-side Copy Paths bro confirms its clipboard write with a toast.
  await openPanelContextMenu(browser, "left");
  await clickContextMenuItem(browser, "Pass to Bro…");
  const copyPathsBro = await browser.$('[data-testid="bro-option"][data-bro-id="builtin:copy-paths"]');
  await copyPathsBro.waitForClickable({ timeout: 10_000 });
  await copyPathsBro.click();
  await broDialog.waitForDisplayed({ timeout: 10_000, reverse: true });
  await waitFor(() => browser.execute(() => (
    /^Copied (full path|\d+ full paths)$/.test(document.querySelector(".transient-hint")?.textContent ?? "")
  )), { timeout: 10_000, label: "Copy Paths bro toast" });
}

async function verifyAboutModal(browser) {
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  // The command palette uses Cmd+Shift+P on macOS and Ctrl+Shift+P elsewhere.
  // Dispatching both is harmless because only the platform binding matches.
  await pressWindowKey(browser, "p", { shift: true, meta: true });
  await pressWindowKey(browser, "p", { shift: true, ctrl: true });

  const paletteInput = await browser.$(".command-palette__input");
  await paletteInput.waitForDisplayed({ timeout: 10_000 });
  await setTextInputValue(browser, ".command-palette__input", "About fbro");
  await browser.keys("Enter");

  const about = await browser.$('[data-testid="about-modal"]');
  await about.waitForDisplayed({ timeout: 10_000 });
  const dialogSemantics = await browser.execute(() => {
    const top = document.querySelector('[data-modal-id="about"][data-modal-state="top"]');
    const dialogs = top?.querySelectorAll('[role="dialog"], [role="alertdialog"]') ?? [];
    return { count: dialogs.length, ariaModal: dialogs[0]?.getAttribute("aria-modal") };
  });
  if (dialogSemantics.count !== 1 || dialogSemantics.ariaModal !== "true") {
    throw new Error(`About modal semantics mismatch: ${JSON.stringify(dialogSemantics)}`);
  }
  const version = await browser.$('[data-testid="about-version"]').getText();
  const commit = await browser.$('[data-testid="about-commit"]').getText();
  const profile = await browser.$('[data-testid="about-profile"]').getText();
  const dirty = await browser.$('[data-testid="about-dirty"]').getText();
  const builtAt = await browser.$('[data-testid="about-built-at"]').getText();
  if (!/^\d+\.\d+\.\d+/.test(version)) throw new Error(`invalid About version: ${version}`);
  if (!/^[0-9a-f]{40}$/.test(commit)) throw new Error(`invalid About commit: ${commit}`);
  if (!["development", "debug", "release"].includes(profile)) throw new Error(`invalid About profile: ${profile}`);
  if (!["Clean", "Dirty", "Unknown"].includes(dirty)) throw new Error(`invalid About dirty state: ${dirty}`);
  if (Number.isNaN(Date.parse(builtAt))) throw new Error(`invalid About build timestamp: ${builtAt}`);

  await browser.keys("Escape");
  await about.waitForDisplayed({ timeout: 10_000, reverse: true });
  await waitFor(async () => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"), {
    timeout: 10_000,
    label: "panel focus after closing About",
  });
}

async function verifyEjectPicker(browser) {
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await pressWindowKey(browser, "p", { shift: true, meta: true });
  await pressWindowKey(browser, "p", { shift: true, ctrl: true });
  await (await browser.$(".command-palette__input")).waitForDisplayed({ timeout: 10_000 });
  await setTextInputValue(browser, ".command-palette__input", "Eject Removable Source");
  await browser.keys("Enter");
  const modal = await browser.$('[data-testid="eject-modal"]');
  await modal.waitForDisplayed({ timeout: 10_000 });
  const state = await browser.execute(() => ({
    top: document.querySelector('[data-modal-id="eject-source"]')?.getAttribute("data-modal-state"),
    focused: document.activeElement?.tagName,
    options: document.querySelectorAll('#eject-source-list option').length,
    disabled: document.querySelector('[data-testid="eject-submit"]')?.hasAttribute("disabled"),
  }));
  if (state.top !== "top" || (state.focused !== "SELECT" && state.focused !== "BUTTON")) {
    throw new Error(`Eject picker focus/layer mismatch: ${JSON.stringify(state)}`);
  }
  if (state.options === 0 && !state.disabled) throw new Error("Eject button enabled without a removable source");
  await browser.keys("Escape");
  await modal.waitForDisplayed({ timeout: 10_000, reverse: true });
  await waitFor(() => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"), {
    timeout: 10_000,
    label: "panel focus after closing Eject picker",
  });
}

async function verifyGridLeftAtFirstItemNavigatesUp(browser, observer, aSrc) {
  const gridDir = path.posix.join(aSrc, "sort-test");
  await goToPanelPath(browser, observer, "left", gridDir);
  const isGrid = () => browser.execute(() =>
    document.querySelector('[data-testid="panel-left"] .panel__list')?.classList.contains("panel__list--grid") === true);
  const cursorCardName = () => browser.execute(() =>
    document.querySelector('[data-testid="panel-left"] .panel__card--cursor')?.getAttribute("data-entry-name") ?? null);
  const firstEntryName = () => browser.execute(() =>
    document.querySelector('[data-testid="panel-left"] .panel__list [data-entry-name]')?.getAttribute("data-entry-name") ?? null);

  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await browser.keys("g");
  await waitFor(isGrid, { timeout: 10_000, label: "grid mode for Left navigation" });

  await browser.keys("End");
  await waitFor(async () => (await cursorCardName()) !== null, {
    timeout: 10_000,
    label: "grid cursor card after End",
  });
  const lastName = await cursorCardName();
  await browser.keys("ArrowLeft");
  await waitFor(async () => {
    const name = await cursorCardName();
    return name !== null && name !== lastName;
  }, { timeout: 10_000, label: "grid Left moved the cursor off the last item" });
  if (observer.state.panels.left?.path !== gridDir) {
    throw new Error(`grid Left away from the first item navigated to ${observer.state.panels.left?.path}`);
  }

  await browser.keys("Home");
  await waitFor(async () => {
    const first = await firstEntryName();
    return first !== null && (await cursorCardName()) === first;
  }, { timeout: 10_000, label: "grid cursor card on the first item" });
  await browser.keys("ArrowLeft");
  await observer.waitForPanelPath("left", aSrc);

  await browser.keys("g");
  await waitFor(async () => !(await isGrid()), {
    timeout: 10_000,
    label: "list mode restored after grid Left navigation",
  });
}

async function verifySelectedFolderInlineNavigation(browser, observer, sortDir, fileName) {
  const selectedNames = () => browser.execute(() => [...document.querySelectorAll('[data-testid="panel-left"] [data-testid="panel-entry"].panel__row--selected')]
    .map((entry) => entry.getAttribute("data-entry-name")));

  await clickEntry(browser, "left", fileName);
  await browser.keys(" ");
  await clickEntry(browser, "left", "selected-folder");
  await browser.keys(" ");
  await waitFor(async () => {
    const selected = await selectedNames();
    return selected.includes("selected-folder") && selected.includes(fileName);
  }, { timeout: 10_000, label: "folder bulk-operation selection" });

  await clickEntry(browser, "left", "selected-folder");
  await browser.keys("ArrowRight");
  await waitFor(() => browser.execute(() => {
    const row = document.querySelector('[data-testid="panel-left"] [data-entry-name="selected-folder"]');
    return row?.querySelector("[data-inline-operation]")?.contains(document.activeElement) === true;
  }), { timeout: 10_000, label: "selected folder inline toolbar focus" });
  if (observer.state.panels.left?.path !== sortDir) {
    throw new Error("Right Arrow navigated into a folder anchoring a multi-selection");
  }

  await browser.keys("Escape");
  await browser.keys("Enter");
  await observer.waitForPanelPath("left", path.posix.join(sortDir, "selected-folder"));
  await goToPanelPath(browser, observer, "left", sortDir);
  await observer.waitForEntry("left", "selected-folder", "directory");
}

async function verifyContextualSelectionOperations(browser, observer, [first, second, third]) {
  const inlineIds = async (name) => browser.execute((name) => {
    const escaped = CSS.escape(name);
    return [...document.querySelectorAll(`[data-testid="panel-left"] [data-entry-name="${escaped}"] [data-inline-operation]`)]
      .map((button) => button.getAttribute("data-inline-operation"));
  }, name);
  const menuLabels = async () => browser.execute(() => [...document.querySelectorAll(".context-menu [role='menuitem'] > span:first-child")]
    .map((label) => label.textContent?.trim() ?? ""));
  const selectedNames = () => browser.execute(() => [...document.querySelectorAll('[data-testid="panel-left"] [data-testid="panel-entry"].panel__row--selected')]
    .map((entry) => entry.getAttribute("data-entry-name")));

  const clearButton = await browser.$('[data-testid="panel-left"] .panel__selection-clear');
  if (await clearButton.isExisting()) await clearButton.click();
  await waitFor(async () => (await selectedNames()).length === 0, { timeout: 10_000, label: "clear contextual selection" });

  await clickEntry(browser, "left", first);
  await browser.keys(" ");
  await clickEntry(browser, "left", second);
  await browser.keys(" ");
  await waitFor(async () => {
    const selected = await selectedNames();
    return selected.includes(first) && selected.includes(second);
  }, { timeout: 10_000, label: "two-item contextual selection" });
  await clickEntry(browser, "left", second);
  await waitFor(async () => {
    const actual = (await inlineIds(second)).join("|");
    if (actual !== "file.compare|panel.pass-to-bro|file.copy|file.move|file.delete") {
      const domCursor = await browser.execute(() => document.querySelector('[data-testid="panel-left"] .panel__row--cursor')?.getAttribute("data-entry-name"));
      const panel = observer.state.panels.left;
      throw new Error(`got ${actual || "<none>"}; DOM cursor=${domCursor}; state cursor=${panel?.cursor}; selectedEntries=${JSON.stringify(panel?.selectedEntries)}`);
    }
    return true;
  }, {
    timeout: 10_000,
    label: "two-item inline contextual operations",
  });
  await openPanelContextMenu(browser, "left");
  const pairLabels = await menuLabels();
  for (const expected of ["Compare", "Pass to Bro…", "Copy", "Move", "Delete", "Copy names", "Copy full paths"]) {
    if (!pairLabels.includes(expected)) throw new Error(`two-item context menu lacks ${expected}: ${JSON.stringify(pairLabels)}`);
  }
  for (const excluded of ["Rename", "Open With…"]) {
    if (pairLabels.includes(excluded)) throw new Error(`two-item context menu unexpectedly includes ${excluded}`);
  }
  const copyItem = await browser.$('[data-contextual-operation="file.copy"]');
  await copyItem.waitForClickable({ timeout: 10_000 });
  await copyItem.click();
  const copyDialog = await browser.$('[data-testid="operation-dialog"]');
  await copyDialog.waitForDisplayed({ timeout: 10_000 });
  const copyDialogText = await copyDialog.getText();
  if (!copyDialogText.includes(first) || !copyDialogText.includes(second)) {
    throw new Error(`selection copy dialog lacks selected entries: ${copyDialogText}`);
  }
  await clickCommand(browser, "file.copy.cancel");

  await clickEntry(browser, "left", third);
  await browser.keys(" ");
  await waitFor(async () => (await selectedNames()).length === 3, { timeout: 10_000, label: "three-item contextual selection" });
  await clickEntry(browser, "left", third);
  await waitFor(async () => (await inlineIds(third)).join("|") === "panel.pass-to-bro|file.copy|file.move|file.delete", {
    timeout: 10_000,
    label: "three-item inline contextual operations",
  });

  await clickEntry(browser, "left", third);
  await browser.keys(" ");
  await waitFor(async () => (await selectedNames()).length === 2, { timeout: 10_000, label: "cursor-wins retained selection" });
  await clickEntry(browser, "left", third);
  await waitFor(async () => (await inlineIds(third)).includes("file.rename"), {
    timeout: 10_000,
    label: "unselected cursor singleton operations",
  });
  await openPanelContextMenu(browser, "left");
  const cursorLabels = await menuLabels();
  if (!cursorLabels.includes("Rename") || !cursorLabels.includes("Copy name") || cursorLabels.includes("Copy names")) {
    throw new Error(`unselected cursor did not win contextual targeting: ${JSON.stringify(cursorLabels)}`);
  }
  await browser.keys("Escape");
  const cleanupButton = await browser.$('[data-testid="panel-left"] .panel__selection-clear');
  await cleanupButton.waitForClickable({ timeout: 10_000 });
  await cleanupButton.click();
  await waitFor(async () => (await selectedNames()).length === 0, { timeout: 10_000, label: "contextual selection cleanup" });
}

async function verifyLiveDateSortCursorAnchor(browser, observer, hostSortDir, nowSec) {
  // Keep the listing shorter than the viewport to verify that virtual tail
  // space still lets new entries grow upward. A non-default row height catches
  // any return to estimated pixel offsets.
  await clickEntry(browser, "left", "baby.txt");
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await browser.performActions([{
    type: "key",
    id: "keyboard",
    actions: [
      { type: "keyDown", value: " " },
      { type: "keyUp", value: " " },
    ],
  }]);
  await browser.releaseActions();
  await waitFor(async () => browser.execute(() => {
    const row = document.querySelector('[data-testid="panel-left"] [data-entry-name="baby.txt"]');
    return row?.classList.contains("panel__row--cursor") === true
      && row.classList.contains("panel__row--selected");
  }), {
    timeout: 10_000,
    label: "live anchor selection",
  });

  observer.sendIntent("SetFontSize", undefined, { value: 18 });
  await waitFor(async () => browser.execute(() => {
    const row = document.querySelector('[data-testid="panel-left"] [data-entry-name="baby.txt"]');
    return row ? Math.round(row.getBoundingClientRect().height) === 27 : false;
  }), { timeout: 10_000, label: "zoomed row height" });

  const before = await browser.execute(() => {
    const list = document.querySelector('[data-testid="panel-left"] .panel__list');
    const row = document.querySelector('[data-testid="panel-left"] [data-entry-name="baby.txt"]');
    return row && list ? {
      offset: row.getBoundingClientRect().top - list.getBoundingClientRect().top,
      alternate: row.classList.contains("panel__row--alt"),
    } : null;
  });
  if (!before) throw new Error("live anchor row disappeared before prepend");
  const newest = path.join(hostSortDir, "newest-live-file.txt");
  fs.writeFileSync(newest, "newest\n");
  fs.utimesSync(newest, nowSec, nowSec + 60);
  await observer.waitForEntry("left", "newest-live-file.txt", "file");

  await waitFor(async () => browser.execute((expected) => {
    const list = document.querySelector('[data-testid="panel-left"] .panel__list');
    const row = document.querySelector('[data-testid="panel-left"] [data-entry-name="baby.txt"]');
    if (!(row instanceof HTMLElement) || !(list instanceof HTMLElement)) {
      throw new Error("baby.txt or its listing viewport is not rendered");
    }
    const actual = {
      offset: row.getBoundingClientRect().top - list.getBoundingClientRect().top,
      alternate: row.classList.contains("panel__row--alt"),
      selected: row.classList.contains("panel__row--selected"),
    };
    if (!actual.selected
      || Math.abs(actual.offset - expected.offset) >= 0.5
      || actual.alternate !== expected.alternate) {
      throw new Error(`expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
    return true;
  }, before), { timeout: 10_000, label: "live date-sort cursor viewport and zebra anchor" });

  const clearSelection = await browser.$('[data-testid="panel-left"] .panel__selection-clear');
  await clearSelection.waitForClickable({ timeout: 10_000 });
  await clearSelection.click();
  fs.unlinkSync(newest);
  await waitFor(() => {
    const entries = observer.state.panels.left?.entries ?? [];
    return entries.some((entry) => entry.name === "newest-live-file.txt") ? false : true;
  }, { timeout: 10_000, label: "live anchor fixture cleanup" });
  observer.sendIntent("SetFontSize", undefined, { value: 13 });
}

async function verifyWindowFocusRestoration(browser, observer) {
  await clickEntry(browser, "left", "aardvark.txt");
  const initialCursor = observer.state.panels.left?.cursor;
  observer.sendIntent("ViewerNavigate", undefined, { panelId: "left", direction: "next" });
  await waitFor(() => observer.state.panels.left?.cursor !== initialCursor, {
    timeout: 10_000,
    label: "viewer file navigation",
  });

  const panelIsActive = () => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left");
  const isGrid = () => browser.execute(() => document.querySelector('[data-testid="panel-left"] .panel__list')?.classList.contains("panel__list--grid") === true);
  const initialGrid = await isGrid();
  const toggleViewModeFromRestoredFocus = async (label) => {
    await waitFor(panelIsActive, { timeout: 10_000, label });
    await browser.keys("g");
    await waitFor(async () => (await isGrid()) !== initialGrid, {
      timeout: 10_000,
      label: `${label}: view mode changed`,
    });
    await browser.keys("g");
    await waitFor(async () => (await isGrid()) === initialGrid, {
      timeout: 10_000,
      label: `${label}: view mode restored`,
    });
  };

  await browser.execute(() => {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    window.dispatchEvent(new FocusEvent("focus"));
  });
  await toggleViewModeFromRestoredFocus("panel focus restored after window reactivation");

  await browser.execute(() => {
    document.querySelector('[data-command-id="file.view"]')?.focus();
    window.dispatchEvent(new FocusEvent("focus"));
  });
  await toggleViewModeFromRestoredFocus("panel focus restored from toolbar button");
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
