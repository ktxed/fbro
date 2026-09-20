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

export async function runLocalFlow(browser, observer) {
  await verifyColorSchemeSwitch(browser, observer);
  await verifyAboutModal(browser);
  await verifyQuitCancelKeyboard(browser);

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

  await goToPanelPath(browser, observer, "left", aSrc);
  await observer.waitForEntry("left", "alpha", "directory");
  await clickEntry(browser, "left", "alpha");
  await clickCommand(browser, "file.copy");
  await expectCheckboxSelected(
    browser,
    '[data-testid="copy-save-digest-file-input"]',
    false,
    "local copy digest checkbox default",
  );
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
  await browser.execute(() => document.querySelector('[data-command-id="file.copy.cancel"]')?.focus());
  await browser.keys("Enter");
  await (await browser.$('[data-testid="operation-dialog"]')).waitForDisplayed({ timeout: 10_000, reverse: true });

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
  await goToPanelPath(browser, observer, "right", path.posix.join(aDst, "alpha"));

  await verifyWindowFocusRestoration(browser, observer);
  await verifyGridLeftAtFirstItemNavigatesUp(browser, observer, aSrc);
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

  await browser.execute(() => document.querySelector('[data-testid="settings-tab-general"]')?.focus());
  await browser.keys("ArrowDown");
  await browser.keys("ArrowDown");
  await waitFor(() => browser.execute(() => {
    const tab = document.querySelector('[data-testid="settings-tab-sharing"]');
    return tab === document.activeElement && tab?.getAttribute("aria-selected") === "true";
  }), { timeout: 10_000, label: "settings roving tab focus" });
  const focusOutline = await browser.execute(() => getComputedStyle(document.activeElement).outlineStyle);
  if (focusOutline === "none") throw new Error("settings tab lacks a visible keyboard focus outline");
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
  await browser.keys("Escape");
  await confirmation.waitForDisplayed({ timeout: 10_000, reverse: true });
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

async function verifyQuitCancelKeyboard(browser) {
  await clickCommand(browser, "app.quit");
  const quit = await browser.$('[data-modal-id="quit"]');
  await quit.waitForDisplayed({ timeout: 10_000 });
  const focusedText = await browser.execute(() => document.activeElement?.textContent?.trim());
  if (focusedText !== "Cancel") throw new Error(`Quit did not focus Cancel: ${focusedText}`);
  await browser.keys("Enter");
  await quit.waitForDisplayed({ timeout: 10_000, reverse: true });
  await (await browser.$('[data-testid="panel-left"]')).waitForDisplayed({ timeout: 10_000 });
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
  for (const expected of ["Compare", "Pass to Bro...", "Copy", "Move", "Delete", "Copy names", "Copy full paths"]) {
    if (!pairLabels.includes(expected)) throw new Error(`two-item context menu lacks ${expected}: ${JSON.stringify(pairLabels)}`);
  }
  for (const excluded of ["Rename", "Open With..."]) {
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
