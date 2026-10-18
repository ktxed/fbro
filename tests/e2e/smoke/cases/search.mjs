import fs from "node:fs";
import path from "node:path";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import { clickCommand, closeTerminalJobDialog, confirmOperation, goToPanelPath, openPanelContextMenu, reloadBrowserApp } from "../browser.mjs";
import { expectModalFocus } from "./modal-focus.mjs";
import { waitFor } from "../wait.mjs";

/** A sparse regular file keeps content search active until truncated or cancelled. */
export async function runSearchActivityFlow(browser, observer) {
  if (process.platform !== "linux") return;
  const root = backendPath("a", "src", "search-activity");
  const fixture = fs.mkdtempSync(`${hostPath(root)}-`);
  const longFolder = "long-folder-".repeat(8);
  const folder = path.join(fixture, longFolder);
  fs.mkdirSync(folder);
  const searchRoot = path.posix.join(`${root}${fixture.slice(hostPath(root).length)}`, longFolder);
  const slowFile = path.join(folder, "slow.txt");
  fs.writeFileSync(path.join(folder, "nonmatch.bin"), "not a name match");
  const openSlowFile = () => {
    const descriptor = fs.openSync(slowFile, "w+");
    fs.ftruncateSync(descriptor, 2 ** 40); // Sparse: no terabyte of disk allocation.
    return descriptor;
  };
  let writer = openSlowFile();
  let jobId;
  const previousScheme = observer.state.settings?.general?.colorScheme ?? "fbro-dark";
  const releaseReader = () => {
    if (writer === null) return;
    fs.ftruncateSync(writer, 0);
    fs.closeSync(writer);
    writer = null;
  };
  const panelHasFocus = () => browser.execute(() =>
    document.activeElement === document.querySelector('[data-testid="panel-left"]'));
  try {
    observer.navigatePanelToPath("left", searchRoot);
    await observer.waitForPanelPath("left", searchRoot);
    await observer.waitForPanelNotShadow("left");
    await reloadBrowserApp(browser, "fbro-a");

    const start = async (background = false) => {
      const input = await openSearch(browser);
      await input.setValue("*.txt");
      await (await browser.$('[data-testid="search-content-input"]')).setValue("absent-needle");
      await clickCommand(browser, background ? "search.background" : "search.start");
      const chip = await browser.$('[data-testid="job-chip"][data-job-kind="search"][data-job-state="Running"]');
      await chip.waitForDisplayed({ timeout: 10_000 });
      jobId = await chip.getAttribute("data-job-id");
      await browser.waitUntil(async () => (await (await chip.$('[data-testid="search-current-folder"]')).getAttribute("title")) === searchRoot, {
        timeout: 10_000, timeoutMsg: "No-hit search did not show its active folder",
      });
      if (!(await chip.getText()).includes("0 hits")) throw new Error("No-hit search reported a match");
    };

    await start();
    const activity = await browser.$('[data-testid="search-dialog"] [data-testid="search-activity"]');
    await activity.waitForDisplayed({ timeout: 10_000 });
    const currentFolder = await browser.$('[data-testid="search-dialog"] [data-testid="search-current-folder"]');
    if ((await currentFolder.getAttribute("title")) !== searchRoot) {
      throw new Error("Search dialog did not show the active no-hit folder");
    }
    const modalBar = await browser.$('[data-testid="search-dialog"] [role="progressbar"]');
    if ((await modalBar.getAttribute("aria-valuenow")) !== "0") {
      throw new Error("Search dialog did not show the backend estimate");
    }
    for (const scheme of ["fbro-dark", "daylight"]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      await waitFor(() => browser.execute(scheme =>
        document.querySelector('.app')?.getAttribute("data-color-scheme") === scheme, scheme), {
        timeout: 10_000, label: `search activity ${scheme} theme`,
      });
      const layout = await browser.execute(() => {
        const folder = document.querySelector('[data-testid="search-dialog"] [data-testid="search-current-folder"]');
        const progress = document.querySelector('[data-testid="search-dialog"] .search-progress__fill');
        return { ellipsis: getComputedStyle(folder).textOverflow, accent: getComputedStyle(progress).backgroundColor };
      });
      if (layout.ellipsis !== "ellipsis" || layout.accent === "rgba(0, 0, 0, 0)") {
        throw new Error(`Incorrect search activity styling in ${scheme}: ${JSON.stringify(layout)}`);
      }
      await browser.saveScreenshot(path.join(artifactsDir, "screenshots", `search-activity-${scheme}.png`));
    }

    // Activity remains visible on Input and Extra, with roving keyboard tabs.
    await browser.execute(() => document.querySelector('#search-tab-results')?.focus());
    await browser.keys("Home");
    await expectModalFocus(browser, "#search-tab-input", "search activity Input tab");
    if (!await activity.isDisplayed()) throw new Error("Search activity disappeared on Input");
    await browser.keys("End");
    await expectModalFocus(browser, "#search-tab-extra", "search activity Extra tab");
    if (!await activity.isDisplayed()) throw new Error("Search activity disappeared on Extra");
    // Hold the reader past another backend activity interval before checking
    // focus. Jobs are connection-local, so this flow inspects the GUI snapshot.
    await browser.execute(async () => { await new Promise(resolve => setTimeout(resolve, 250)); });
    await expectModalFocus(browser, "#search-tab-extra", "search updates preserve tab focus");

    await browser.keys("Escape");
    await (await browser.$('[data-testid="search-dialog"]')).waitForDisplayed({ timeout: 10_000, reverse: true });
    await waitFor(panelHasFocus, { timeout: 10_000, label: "background search restores panel focus" });
    const chip = await browser.$(`[data-testid="job-chip"][data-job-id="${jobId}"]`);
    const chipFolder = await chip.$('[data-testid="search-current-folder"]');
    if ((await chipFolder.getAttribute("title")) !== searchRoot) {
      throw new Error("Background search chip did not show its active folder");
    }
    if ((await (await chip.$('[role="progressbar"]')).getAttribute("aria-valuenow")) !== "0") {
      throw new Error("Background search chip did not show its estimate");
    }
    // Keyboard activation reopens the job at the same backend estimate.
    await browser.execute(jobId => document.querySelector(`[data-job-id="${jobId}"]`)?.focus(), jobId);
    await browser.keys("Enter");
    await activity.waitForDisplayed({ timeout: 10_000 });
    await browser.execute(() => document.querySelector('[data-command-id="search.cancel"]')?.focus());
    await browser.keys("Enter");
    releaseReader();
    await browser.waitUntil(async () => (await chip.getAttribute("data-job-state")) === "Cancelled", {
      timeout: 10_000, timeoutMsg: "Search activity did not stop after keyboard cancellation",
    });
    if ((await (await chip.$('[role="progressbar"]')).getAttribute("aria-valuenow")) !== "0") {
      throw new Error("Cancelled folder incorrectly advanced the estimate");
    }
    await clickCommand(browser, "search.close");
    if (await chip.$('.search-progress__activity').isExisting()) {
      throw new Error("Cancelled search kept animating");
    }

    writer = openSlowFile();
    await start(true);
    await waitFor(panelHasFocus, { timeout: 10_000, label: "direct background search preserves panel focus" });
    releaseReader();
    const completedChip = await browser.$(`[data-testid="job-chip"][data-job-id="${jobId}"]`);
    await browser.waitUntil(async () => (await completedChip.getAttribute("data-job-state")) === "Completed", {
      timeout: 10_000, timeoutMsg: "No-hit search did not complete",
    });
    await browser.waitUntil(async () => (await (await completedChip.$('[role="progressbar"]')).getAttribute("aria-valuenow")) === "100", {
      timeout: 10_000, timeoutMsg: "Completed search chip did not reach 100 percent",
    });
    if (await completedChip.$('.search-progress__activity').isExisting()) {
      throw new Error("Completed search kept animating");
    }
    await completedChip.click();
    const summary = await browser.$('[data-testid="search-summary"]');
    await summary.waitForDisplayed({ timeout: 10_000 });
    if (!(await summary.getText()).includes("0 results found, 2 items scanned")) {
      throw new Error(`Incorrect final no-hit counters: ${await summary.getText()}`);
    }
    await clickCommand(browser, "search.close");
  } finally {
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    releaseReader();
    if (jobId) observer.sendIntent("CancelJob", undefined, { jobId });
    fs.rmSync(fixture, { recursive: true, force: true });
  }
}

export async function runSearchFlow(browser, observer) {
  // Keep duplicate-name fixtures separate from files created by earlier flows.
  const aDst = backendPath("a", "dst", "search-viewer-fixtures");
  const hostDst = hostPath(aDst);
  fs.mkdirSync(hostDst, { recursive: true });
  fs.writeFileSync(path.join(hostDst, "renamed.txt"), "root result\n");
  for (const directory of ["hint-one", "hint-two"]) {
    fs.mkdirSync(path.join(hostDst, directory), { recursive: true });
    fs.writeFileSync(path.join(hostDst, directory, "renamed.txt"), `${directory}\n`);
  }
  fs.writeFileSync(path.join(hostDst, "uniquely-renamed.txt"), "unique result\n");
  observer.navigatePanelToPath("left", aDst);
  await observer.waitForPanelPath("left", aDst);
  await observer.waitForPanelNotShadow("left");
  await reloadBrowserApp(browser, "fbro-a");

  const leftPanel = await browser.$('[data-testid="panel-left"]');
  await leftPanel.click();
  await browser.keys(["f"]);
  const quickFilterInput = await browser.$('[data-testid="panel-left"] .panel__filter-input');
  await quickFilterInput.waitForDisplayed({ timeout: 10_000 });
  await quickFilterInput.setValue("does-not-match-search-result");
  await observer.waitForNoEntry("left", "renamed.txt");

  let input = await openSearch(browser);
  await browser.execute(() => document.querySelector('#search-tab-input')?.focus());
  await browser.keys("ArrowRight");
  await browser.keys("ArrowRight");
  await browser.waitUntil(async () => browser.execute(() => {
    const tab = document.querySelector('#search-tab-extra');
    return tab === document.activeElement && tab?.getAttribute("aria-selected") === "true";
  }), { timeout: 10_000, timeoutMsg: "Search tab focus did not follow ArrowRight" });
  await browser.keys("Home");
  await browser.waitUntil(async () => browser.execute(() => document.activeElement?.id === "search-tab-input"), {
    timeout: 10_000,
    timeoutMsg: "Search Home key did not restore the first tab",
  });
  await browser.execute(() => document.querySelector('[data-command-id="search.close"]')?.focus());
  await browser.keys("Enter");
  await (await browser.$('[data-testid="search-dialog"]')).waitForDisplayed({ timeout: 10_000, reverse: true });
  input = await openSearch(browser);
  await browser.execute(() => document.querySelector('[data-testid="search-file-mask-input"]')?.focus());
  await browser.execute(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "P",
      code: "KeyP",
      ctrlKey: true,
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    }));
  });
  if (await browser.$(".command-palette").isExisting()) {
    throw new Error("Command Palette opened above the Search modal");
  }
  if ((await browser.$$('[data-modal-state="top"]')).length !== 1) {
    throw new Error("Search modal was not the sole active modal layer");
  }
  await input.setValue("*renamed*");
  await clickCommand(browser, "search.start");
  await clickCommand(browser, "search.to-results");
  await observer.waitForSearchResultsMounted("left");
  await browser.waitUntil(
    async () => {
      const panel = observer.state.panels.left;
      return panel?.filter == null && panel?.filterModeActive === false;
    },
    {
      timeout: 10_000,
      timeoutMsg: "Quick filter remained active after mounting search results",
    },
  );
  if (await quickFilterInput.isDisplayed()) {
    throw new Error("Quick filter bar remained visible after mounting search results");
  }
  await observer.waitForEntry("left", "renamed.txt", "file");
  await observer.waitForEntry("left", "uniquely-renamed.txt", "file");
  await browser.waitUntil(async () => {
    const hints = await browser.execute(() => Array.from(
      document.querySelectorAll('[data-testid="panel-left"] [data-entry-name="renamed.txt"]')
    ).map((row) => row.querySelector('[data-testid="path-hint"]')?.textContent?.trim() ?? ""));
    if (hints.length < 3) return false;
    return hints.every(Boolean)
      && hints.some((hint) => hint.includes("hint-one"))
      && hints.some((hint) => hint.includes("hint-two"));
  }, {
    timeout: 10_000,
    timeoutMsg: "Duplicate search results did not receive distinguishing path hints",
  });
  const uniqueHint = await browser.$('[data-testid="panel-left"] [data-entry-name="uniquely-renamed.txt"] [data-testid="path-hint"]');
  if (await uniqueHint.isExisting()) {
    throw new Error("Unique search result unexpectedly received a path hint");
  }
  const hintedRow = (await browser.$$('[data-testid="panel-left"] [data-entry-name="renamed.txt"]'))[0];
  const hintedRowTitle = await hintedRow.getAttribute("title");
  if (!hintedRowTitle?.endsWith("/renamed.txt")) {
    throw new Error(`Duplicate search result did not expose its full path as a tooltip: ${hintedRowTitle}`);
  }
  await browser.waitUntil(async () => {
    const dateCell = await hintedRow.$('.panel__mtime');
    if (!await dateCell.isExisting()) return false;
    const text = await dateCell.getText();
    return text.trim().length > 0;
  }, {
    timeout: 5_000,
    timeoutMsg: "Mounted search result Date column stayed empty",
  });
  await verifyDuplicateResultViewer(browser, observer);
}

async function verifyDuplicateResultViewer(browser, observer) {
  await observer.waitForJobIdle();
  observer.sendIntent("SetSort", "left", { by: "name", order: "asc" });
  await observer.waitForSort("left", "name", "asc");
  const panel = observer.state.panels.left;
  const hits = panel.entries.map((entry, index) => ({ ...entry, index }))
    .filter(entry => entry.name === "renamed.txt");
  if (hits.length !== 3) throw new Error(`Expected 3 duplicate viewer fixtures, got ${hits.length}`);

  const mainUrl = await browser.getUrl();
  observer.sendIntent("SetCursor", "left", { index: hits[1].index });
  observer.sendIntent("RequestStateSnapshot");
  await browser.waitUntil(() => observer.state.panels.left.cursor === hits[1].index, { timeout: 10_000 });
  // The hybrid harness renders the viewer in the host window, since backend
  // native windows live in Docker. Navigation still uses the real viewer WS.
  const params = new URLSearchParams({ viewer: "1", path: hits[1].sourcePath,
    source: panel.sourceId, name: hits[1].name, label: "main", panel: "left" });
  const expectContent = async (hit) => {
    const expected = fs.readFileSync(hostPath(hit.sourcePath), "utf8").trim();
    await browser.waitUntil(async () => browser.execute(expected => {
      const content = document.querySelector(".viewer__content")?.textContent
        ?? Array.from(document.querySelectorAll(".viewer__content-cell"))
          .map(cell => cell.textContent).join("\n");
      return content.trim() === expected;
    }, expected), {
      timeout: 10_000,
      timeoutMsg: `Viewer did not display the actual search result ${hit.sourcePath}`,
    });
  };
  try {
    await browser.url(`${mainUrl.split("?")[0]}?${params}`);
    await expectContent(hits[1]);
    for (const [key, hit] of [["j", hits[2]], ["k", hits[1]], ["k", hits[0]], ["j", hits[1]]]) {
      await browser.keys(key);
      await expectContent(hit);
    }
  } finally {
    await browser.url(mainUrl);
    await browser.$('[data-testid="panel-left"]').waitForDisplayed({ timeout: 10_000 });
  }
}

// A registered desktop application records launches without opening a real editor.
function installSearchOpenWithFixture() {
  const stateRoot = path.join(artifactsDir, "fbro-a", "state");
  const appDir = path.join(stateRoot, "home", ".local", "share", "applications");
  const configDir = path.join(stateRoot, "config");
  const script = backendPath("a", "src", "search-open-with.sh");
  const launchLog = backendPath("a", "src", "search-open-with.log");
  const desktopId = "fbro-search-smoke.desktop";
  fs.mkdirSync(appDir, { recursive: true });
  fs.mkdirSync(configDir, { recursive: true });
  fs.writeFileSync(hostPath(script), `#!/bin/sh\nprintf '%s\\n' "$1" >> '${launchLog}'\n`, { mode: 0o755 });
  const files = new Map([
    [path.join(appDir, desktopId), `[Desktop Entry]\nType=Application\nName=fbro search smoke opener\nExec=${script} %f\nMimeType=text/plain;inode/directory;\nTerminal=false\n`],
    [path.join(appDir, "mimeinfo.cache"), `[MIME Cache]\ntext/plain=${desktopId};\ninode/directory=${desktopId};\n`],
    [path.join(configDir, "mimeapps.list"), `[Default Applications]\ntext/plain=${desktopId};\ninode/directory=${desktopId};\n`],
  ]);
  const previous = new Map([...files.keys()].map(file => [file, fs.existsSync(file) ? fs.readFileSync(file) : null]));
  for (const [file, content] of files) fs.writeFileSync(file, content);
  return {
    launchLog: hostPath(launchLog),
    cleanup() {
      for (const [file, content] of previous) {
        if (content === null) fs.rmSync(file, { force: true });
        else fs.writeFileSync(file, content);
      }
      fs.rmSync(hostPath(script), { force: true });
    },
  };
}

export async function runSearchActionsFlow(browser, observer) {
  const root = backendPath("a", "dst", "search-actions");
  for (const folder of ["one", "two"]) {
    fs.mkdirSync(hostPath(path.posix.join(root, folder)), { recursive: true });
    fs.writeFileSync(hostPath(path.posix.join(root, folder, "result.txt")), `${folder}\n`);
  }
  const directory = path.posix.join(root, "result-folder");
  fs.mkdirSync(hostPath(directory), { recursive: true });
  const deletedPath = path.posix.join(root, "result-gone.txt");
  const hiddenPath = path.posix.join(root, "two", ".result.txt");
  fs.writeFileSync(hostPath(hiddenPath), "hidden result\n");
  fs.writeFileSync(hostPath(deletedPath), "remove after searching\n");
  const fixture = installSearchOpenWithFixture();
  const previousScheme = observer.state.settings?.general?.colorScheme ?? "fbro-dark";
  const otherPath = observer.state.panels.right.path;
  const panelSelector = '[data-testid="panel-left"]';
  const jumpSelector = `${panelSelector} [data-inline-operation="panel.jump-to-location"]`;
  const dialogSelector = '[data-testid="open-with-dialog"]';
  const panelHasFocus = () => browser.execute(selector => document.activeElement === document.querySelector(selector), panelSelector);
  const launches = () => fs.existsSync(fixture.launchLog)
    ? fs.readFileSync(fixture.launchLog, "utf8").trim().split("\n").filter(Boolean) : [];
  const waitForState = (condition, label) => {
    const start = observer.messages.length;
    return waitFor(() => {
      observer.sendIntent("RequestStateSnapshot");
      return observer.messages.slice(start).some(message => message.type === "StateSnapshot") && condition();
    }, { timeout: 10_000, interval: 250, label });
  };
  try {
    observer.navigatePanelToPath("left", root);
    await observer.waitForPanelPath("left", root);
    await observer.waitForPanelNotShadow("left");
    await reloadBrowserApp(browser, "fbro-a");
    await (await openSearch(browser)).setValue("*result*");
    await clickCommand(browser, "search.start");
    await clickCommand(browser, "search.to-results");
    await observer.waitForSearchResultsMounted("left");
    await observer.waitForJobIdle();
    const sourceId = observer.state.panels.left.sourceId;
    const jobId = sourceId.slice("search:".length);
    const remount = async () => {
      await browser.$(`[data-testid="job-chip"][data-job-id="${jobId}"]`).click();
      await clickCommand(browser, "search.to-results");
      await observer.waitForSearchResultsMounted("left");
      await observer.waitForPanelReady("left");
      await browser.waitUntil(async () => browser.execute(() =>
        [...document.querySelectorAll('[data-testid="panel-left"] [data-entry-name="result.txt"]')].length === 2
      ), { timeout: 10_000, timeoutMsg: "Search results did not remount" });
    };
    const choose = async fullPath => {
      const index = observer.state.panels.left.entries.findIndex(entry => entry.sourcePath === fullPath);
      if (index < 0) throw new Error(`Missing search result ${fullPath}`);
      const row = await browser.$(`${panelSelector} [data-abs-index="${index}"]`);
      await row.waitForDisplayed({ timeout: 10_000 });
      await row.click();
      await browser.waitUntil(async () => (await row.getAttribute("class"))?.includes("panel__row--cursor"), {
        timeout: 10_000, timeoutMsg: `Cursor did not reach ${fullPath}`,
      });
      await waitForState(() => observer.state.panels.left.cursor === index, `cursor snapshot for ${fullPath}`);
      return index;
    };
    const checkReconnectRescan = async (cursor, selectedPaths) => {
      const version = observer.state.panels.left.version;
      // Reconnection forces a quiet replacement scan, including when no
      // filesystem event happens to race the click in this particular run.
      await reloadBrowserApp(browser, "fbro-a");
      await waitForState(() => {
        const panel = observer.state.panels.left;
        return panel.version > version && panel.status === "ready"
          && panel.cursor === cursor
          && JSON.stringify(panel.selectedEntries.map(entry => entry.sourcePath)) === JSON.stringify(selectedPaths);
      }, "search rescan preserves exact cursor and selected paths");
      await browser.waitUntil(() => browser.execute(({ panelSelector, cursor }) =>
        document.querySelector(`${panelSelector} .panel__row--cursor`)?.getAttribute("data-abs-index") === String(cursor),
      { panelSelector, cursor }), { timeout: 10_000, timeoutMsg: "Search rescan did not render the preserved cursor" });
    };
    const selectDuplicates = async () => {
      const clear = await browser.$(`${panelSelector} .panel__selection-clear`);
      if (await clear.isExisting()) {
        await clear.click();
        await waitForState(() => observer.state.panels.left.selectedEntries.length === 0, "reset duplicate selection");
      }
      for (const [index, fullPath] of [firstPath, secondPath].entries()) {
        await choose(fullPath);
        await browser.keys(" ");
        await waitForState(() => observer.state.panels.left.selectedEntries.length === index + 1, "select duplicate fixture");
      }
    };
    const openWith = async (fullPath, keyboard = true) => {
      await choose(fullPath);
      await openPanelContextMenu(browser, "left");
      const item = await browser.$('[data-contextual-operation="platform.open-with"]');
      await item.waitForDisplayed({ timeout: 10_000 });
      if (keyboard) {
        const steps = await browser.execute(() => [...document.querySelectorAll('.context-menu [role="menuitem"]')]
          .findIndex(item => item.getAttribute("data-contextual-operation") === "platform.open-with"));
        for (let step = 0; step < steps; step++) await browser.keys("ArrowDown");
        await browser.keys("Enter");
      } else await item.click();
      await browser.$(dialogSelector).waitForDisplayed({ timeout: 10_000 });
      await expectModalFocus(browser, `${dialogSelector} [role="listbox"]`, "search Open With initial focus");
      await browser.waitUntil(async () => browser.execute(() => [...document.querySelectorAll('[data-testid="open-with-dialog"] [role="option"]')]
        .some(option => option.textContent.includes("fbro search smoke opener"))), {
        timeout: 10_000, timeoutMsg: "Registered search fixture application was not offered",
      });
    };

    const firstPath = path.posix.join(root, "one", "result.txt");
    const secondPath = path.posix.join(root, "two", "result.txt");

    // Copy the second same-name result through the bottom bar, with no selection.
    const copyDestination = path.posix.join(root, "copy-destination");
    fs.mkdirSync(hostPath(copyDestination), { recursive: true });
    await goToPanelPath(browser, observer, "right", copyDestination);
    try {
      await choose(secondPath);
      const copy = await browser.$('.toolbar [data-command-id="file.copy"]');
      if (!await copy.isEnabled()) throw new Error("Toolbar Copy was disabled on search results");
      await copy.click();
      await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "search toolbar Copy focus");
      await browser.$('[data-testid="file-conflict-policy"]').selectByAttribute("value", "overwrite");
      await confirmOperation(browser, "file.copy.confirm");
      await observer.waitForJobIdle();
      await browser.$('[data-command-id="job.close"]').waitForDisplayed({ timeout: 10_000 });
      const copied = fs.readFileSync(hostPath(path.posix.join(copyDestination, "result.txt")), "utf8");
      if (copied !== "two\n") throw new Error(`Toolbar Copy used the wrong duplicate: ${copied}`);
      await closeTerminalJobDialog(browser);
      await browser.waitUntil(panelHasFocus, { timeout: 10_000, timeoutMsg: "Search Copy did not restore listing focus" });
    } finally {
      observer.navigatePanelToPath("right", otherPath);
      await observer.waitForPanelPath("right", otherPath);
    }

    const checkSelectedCopy = async (count, keyboard) => {
      if (keyboard) await browser.keys("F5");
      else await clickCommand(browser, "file.copy");
      await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "selected search Copy focus");
      const summary = await browser.$('[data-testid="operation-dialog"] .modal__summary').getText();
      if (!summary.includes(count === 1 ? "result.txt" : `${count} items`)) {
        throw new Error(`Search Copy lost its selection: ${summary}`);
      }
      const cursor = observer.state.panels.left.cursor;
      await browser.keys("ArrowDown");
      await waitForState(() => observer.state.panels.left.cursor === cursor, "Copy covers search listing");
      await browser.keys("Escape");
      await browser.$('[data-testid="operation-dialog"]').waitForDisplayed({ timeout: 10_000, reverse: true });
      await browser.waitUntil(panelHasFocus, { timeout: 10_000, timeoutMsg: "Cancelled search Copy did not restore focus" });
    };
    await choose(firstPath);
    await browser.keys(" ");
    await waitForState(() => observer.state.panels.left.selectedEntries.length === 1, "first search selection");
    await choose(secondPath);
    await checkSelectedCopy(1, true);
    await choose(firstPath);
    await browser.$(jumpSelector).waitForDisplayed({ timeout: 10_000 });
    await openPanelContextMenu(browser, "left");
    await browser.$('[data-contextual-operation="platform.open-with"]').waitForDisplayed({ timeout: 10_000 });
    await browser.keys("Escape");
    await choose(secondPath);
    await browser.keys(" ");
    await waitForState(() => observer.state.panels.left.selectedEntries.length === 2, "two search selections");
    await checkSelectedCopy(2, false);
    await checkSelectedCopy(2, true);

    // Both entry points use the same backend duplicate policy. Each operation
    // starts with exclusion, even after a previous copy chose suffixes.
    const duplicatePolicySelector = '[data-testid="copy-duplicate-name-policy"]';
    for (const [scheme, policy, inline, background] of [
      ["fbro-dark", "suffix", true, true],
      ["fbro-dark", "exclude", false, false],
      ["daylight", "suffix", false, false],
      ["daylight", "exclude", true, false],
    ]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      await waitForState(() => observer.state.settings.general.colorScheme === scheme, `duplicate Copy ${scheme}`);
      const destination = path.posix.join(root, `copy-${scheme}-${policy}`);
      fs.mkdirSync(hostPath(destination), { recursive: true });
      await goToPanelPath(browser, observer, "right", destination);
      try {
        // Completed file operations clear selection. Each policy scenario
        // establishes its own two-file input before opening a confirmation.
        await selectDuplicates();
        const firstSource = observer.state.panels.left.selectedEntries[0]?.sourcePath;
        if (!firstSource) throw new Error("Selected search result lost its source path");
        if (inline) await browser.$(`${panelSelector} [data-inline-operation="file.copy"]`).click();
        else await clickCommand(browser, "file.copy");
        await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "duplicate Copy initial focus");
        const policyControl = await browser.$(duplicatePolicySelector);
        if (await policyControl.getValue() !== "exclude") throw new Error("Duplicate Copy did not default to exclusion");
        const listedPaths = await browser.$('[data-testid="operation-dialog"] .modal__file-list').getText();
        if (!listedPaths.includes(firstPath) || !listedPaths.includes(secondPath)) {
          throw new Error(`Duplicate confirmation hid source paths: ${listedPaths}`);
        }
        await browser.keys("u");
        await expectModalFocus(browser, duplicatePolicySelector, "duplicate policy mnemonic");
        if (policy === "suffix") {
          await browser.keys("ArrowDown");
          await browser.waitUntil(async () => await policyControl.getValue() === "suffix", { timeout: 5_000 });
        }
        await browser.execute(() => document.querySelector('[data-command-id="file.copy.confirm"]')?.focus());
        const cursor = observer.state.panels.left.cursor;
        await browser.keys("ArrowDown");
        await waitForState(() => observer.state.panels.left.cursor === cursor, "duplicate Copy covers listing");
        if (background) {
          const paused = await browser.$('[data-testid="file-operation-start-paused"]');
          if (await paused.isSelected()) await paused.click();
          await clickCommand(browser, "file.copy.background");
        } else await confirmOperation(browser, "file.copy.confirm");
        await waitFor(() => {
          const names = fs.readdirSync(hostPath(destination));
          return names.length === (policy === "suffix" ? 2 : 1)
            && names.every(name => fs.readFileSync(hostPath(path.posix.join(destination, name)), "utf8").length === 4);
        }, { timeout: 10_000, label: `${policy} search Copy contents` });
        const names = fs.readdirSync(hostPath(destination));
        const original = fs.readFileSync(hostPath(path.posix.join(destination, "result.txt")), "utf8");
        if (original !== fs.readFileSync(hostPath(firstSource), "utf8")) throw new Error("Duplicate Copy did not retain the first source");
        if (policy === "suffix") {
          const renamed = names.find(name => /^result-[0-9a-f]{4}\.txt$/.test(name));
          if (!renamed) throw new Error(`Invalid duplicate suffix: ${names}`);
          if (fs.readFileSync(hostPath(path.posix.join(destination, renamed)), "utf8") === original) {
            throw new Error("Suffix Copy copied the same source twice");
          }
        }
        await observer.waitForJobIdle();
        if (!background) {
          await browser.$('[data-command-id="job.close"]').waitForDisplayed({ timeout: 10_000 });
          await closeTerminalJobDialog(browser);
        }
        await browser.waitUntil(async () => browser.execute(selector => {
          const active = document.activeElement;
          return active === document.querySelector(selector)
            || active?.matches('[data-inline-operation="file.copy"]');
        }, panelSelector), { timeout: 10_000, timeoutMsg: "Duplicate Copy did not restore its opener" });
      } finally {
        observer.navigatePanelToPath("right", otherPath);
        await observer.waitForPanelPath("right", otherPath);
      }
    }
    await selectDuplicates();
    if (await browser.$(jumpSelector).isExisting()) throw new Error("Multi-selection exposed Jump to location");
    await openPanelContextMenu(browser, "left");
    if (await browser.$('[data-contextual-operation="platform.open-with"]').isExisting()) {
      throw new Error("Multi-selection exposed Open With");
    }
    await browser.keys("Escape");
    await browser.$(`${panelSelector} .panel__selection-clear`).click();
    await waitForState(() => observer.state.panels.left.selectedEntries.length === 0, "cleared search selection");

    for (const [scheme, fullPath, keyboard] of [["fbro-dark", firstPath, true], ["daylight", secondPath, false]]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      await waitForState(() => observer.state.settings.general.colorScheme === scheme, `search actions ${scheme}`);
      await openWith(fullPath, keyboard);
      const cursor = observer.state.panels.left.cursor;
      await browser.keys("ArrowDown");
      await waitForState(() => observer.state.panels.left.cursor === cursor, "covered search cursor");
      await browser.keys("Escape");
      await browser.$(dialogSelector).waitForDisplayed({ timeout: 10_000, reverse: true });
      await browser.waitUntil(panelHasFocus, { timeout: 10_000, timeoutMsg: "Open With did not return listing focus" });
      await choose(fullPath);
      const jump = await browser.$(jumpSelector);
      await jump.waitForDisplayed({ timeout: 10_000 });
      if (keyboard) {
        await browser.keys("ArrowRight");
        await browser.waitUntil(async () => browser.execute(selector =>
          document.activeElement === document.querySelector(selector), jumpSelector), {
          timeout: 10_000, timeoutMsg: "Right Arrow did not focus Jump to location",
        });
        await browser.keys("Enter");
      } else await jump.click();
      await observer.waitForPanelPath("left", path.posix.dirname(fullPath));
      await observer.waitForPanelNotShadow("left");
      await observer.waitForPanelReady("left");
      await browser.waitUntil(() => observer.state.panels.left.entries[observer.state.panels.left.cursor]?.name === "result.txt", {
        timeout: 10_000, timeoutMsg: "Jump did not place cursor on result file",
      });
      await browser.waitUntil(panelHasFocus, { timeout: 10_000, timeoutMsg: "Jump did not retain listing focus" });
      if (observer.state.panels.right.path !== otherPath) throw new Error("Jump changed the other panel");
      await remount();
    }

    await openWith(secondPath);
    const beforeLaunch = launches().length;
    const fixtureIndex = await browser.execute(() => [...document.querySelectorAll('[data-testid="open-with-dialog"] [role="option"]')]
      .findIndex(option => option.textContent.includes("fbro search smoke opener")));
    for (let step = 0; step < fixtureIndex; step++) await browser.keys("ArrowDown");
    await browser.keys("Enter");
    await browser.waitUntil(() => launches().length === beforeLaunch + 1, { timeout: 10_000, timeoutMsg: "Open With did not launch exactly once" });
    if (launches().at(-1) !== secondPath) throw new Error(`Open With launched the wrong duplicate: ${launches().at(-1)}`);
    await browser.waitUntil(panelHasFocus, { timeout: 10_000 });

    await openWith(directory, false);
    await browser.keys("Escape");
    await browser.waitUntil(panelHasFocus, { timeout: 10_000 });
    await browser.execute(() => {
      const send = WebSocket.prototype.send;
      WebSocket.prototype.send = function (data) {
        WebSocket.prototype.send = send;
        window.__searchActionsSocket = this;
        return send.call(this, data);
      };
    });
    await openWith(firstPath);
    // Reversing the sort swaps same-name results at the saved absolute index.
    observer.sendIntent("SetSort", "left", { by: "name", order: "desc" });
    await observer.waitForSort("left", "name", "desc");
    await observer.waitForPanelReady("left");
    // Deliver the changed backend snapshot on the picker's own GUI connection.
    await browser.execute(() => window.__searchActionsSocket.send(JSON.stringify({
      type: "Intent", timestamp: Date.now(), name: "RequestStateSnapshot", args: {},
    })));
    await browser.$(dialogSelector).waitForDisplayed({ timeout: 10_000, reverse: true });
    await browser.waitUntil(panelHasFocus, { timeout: 10_000, timeoutMsg: "Stale picker did not return focus" });

    fs.rmSync(hostPath(deletedPath));
    await choose(deletedPath);
    await browser.$(jumpSelector).click();
    await browser.waitUntil(async () => {
      const text = await browser.$('[data-testid="transient-hint"]').getText().catch(() => "");
      return text.includes(deletedPath);
    }, {
      timeout: 10_000, timeoutMsg: "Deleted search result was not rejected",
    });
    if (observer.state.panels.left.sourceId !== sourceId) throw new Error("Rejected jump discarded search results");
    await browser.waitUntil(panelHasFocus, { timeout: 10_000, timeoutMsg: "Rejected jump lost listing focus" });
    if (launches().length !== beforeLaunch + 1) throw new Error("Open With launched more than once");

    // Run reload checks after all job-chip remounts: initial snapshots do not
    // replay the completed jobs used by those earlier steps.
    const secondIndex = await choose(secondPath);
    await checkReconnectRescan(secondIndex, []);
    const firstIndex = await choose(firstPath);
    await browser.keys(" ");
    await waitForState(() => observer.state.panels.left.selectedEntries.length === 1, "selection before rescan");
    await checkReconnectRescan(firstIndex, [firstPath]);
    await browser.$(`${panelSelector} .panel__selection-clear`).click();
    await waitForState(() => observer.state.panels.left.selectedEntries.length === 0, "clear rescan selection");

    await choose(hiddenPath);
    await browser.$(jumpSelector).click();
    await observer.waitForPanelPath("left", path.posix.dirname(hiddenPath));
    await observer.waitForPanelReady("left");
    await waitForState(() => {
      const panel = observer.state.panels.left;
      return panel.showHidden && panel.entries[panel.cursor]?.name === ".result.txt";
    }, "hidden search result cursor");
    await browser.waitUntil(panelHasFocus, { timeout: 10_000 });
    console.log("Search actions passed: duplicate jumps, Open With launch, selection, stale targets, focus, both themes.");
  } finally {
    await browser.execute(() => { delete window.__searchActionsSocket; }).catch(() => {});
    fixture.cleanup();
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
  }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async function openSearch(browser) {
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await browser.execute(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "f",
      code: "KeyF",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    }));
  });
  const input = await browser.$('[data-testid="search-file-mask-input"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await expectModalFocus(browser, '[data-testid="search-file-mask-input"]', "search mask initial focus");
  return input;
}

/**
 * Focus the `year` segment of a DateTimeInput identified by its aria-label and
 * type the given digits.
 */
async function setDateTimeYear(browser, ariaLabel, yearStr) {
  await browser.execute((ariaLabel) => {
    const container = document.querySelector(`div[aria-label="${ariaLabel}"]`);
    if (!container) throw new Error(`DateTimeInput aria-label="${ariaLabel}" not found`);
    const yearSeg = container.querySelector('span[aria-label="year"]');
    if (!yearSeg) throw new Error(`year segment not found within "${ariaLabel}"`);
    yearSeg.focus();
  }, ariaLabel);
  for (const ch of yearStr) {
    await browser.keys([ch]);
  }
}

/**
 * Open and configure the search modal with the given date-field filter and
 * a "Between" interval covering current year ±1, start the search, mount
 * results, and return whether `checkFile` appears in the result panel.
 */
async function runDatetimeIntervalSearch(browser, observer, dateField, sortDir, checkFile) {
  observer.navigatePanelToPath("left", sortDir);
  await observer.waitForPanelPath("left", sortDir);
  await observer.waitForPanelNotShadow("left");
  await reloadBrowserApp(browser, "fbro-a");

  // File mask: "*" (match all)
  const maskInput = await openSearch(browser);
  await maskInput.setValue("*");

  // Switch to Extra tab (third tab: Input / Results / Extra)
  const extraTab = await browser.$('.search-modal__tab:nth-child(3)');
  await extraTab.waitForDisplayed({ timeout: 5_000 });
  await extraTab.click();

  // Enable File datetime filter
  const timeCheckbox = await browser.$('.search-modal__section--last input[type="checkbox"]');
  await timeCheckbox.waitForDisplayed({ timeout: 5_000 });
  await timeCheckbox.click();

  // Select "Between" radio — wait for it to appear after checkbox click
  const betweenRadio = await browser.$('.search-modal__radio-options label:nth-child(3) input[type="radio"]');
  await betweenRadio.waitForDisplayed({ timeout: 5_000 });
  await betweenRadio.click();

  // Toggle Created / Modified segmented button
  const segBtnIdx = dateField === "created" ? 0 : 1;
  const segBtn = await browser.$(`.search-modal__seg-btn:nth-child(${segBtnIdx + 1})`);
  await segBtn.waitForDisplayed({ timeout: 5_000 });
  await segBtn.click();

  // Set tight date range: current year ±1
  const now = new Date();
  const fromYear = String(now.getFullYear() - 1);
  const toYear = String(now.getFullYear() + 1);
  await setDateTimeYear(browser, "Between from date", fromYear);
  await setDateTimeYear(browser, "Between to date", toYear);

  // Run search
  await clickCommand(browser, "search.start");
  await clickCommand(browser, "search.to-results");
  await observer.waitForSearchResultsMounted("left");

  // Check for the specific file
  if (checkFile) {
    try {
      await observer.waitForEntry("left", checkFile, "file", 10_000);
      return true;
    } catch {
      return false;
    }
  }

  // If no specific file requested, just count the three sort-test files
  let found = 0;
  for (const name of ["aardvark.txt", "zebra.txt", "baby.txt"]) {
    try {
      await observer.waitForEntry("left", name, "file", 10_000);
      found++;
    } catch {
      // Some filesystems don't support birth-time, so "created" mode may
      // yield fewer results.
    }
  }
  return found;
}

// ─── Datetime search flow ────────────────────────────────────────────────────

export async function runDatetimeSearchFlow(browser, observer) {
  const sortDir = path.posix.join(backendPath("a", "src"), "sort-test");
  const hostSortDir = hostPath(sortDir);

  // Create a file whose modification time is set to two years ago, well
  // outside the current year ±1 search interval.  Birth time (btime /
  // creation time) is set by the filesystem at creation time and cannot be
  // changed afterwards — it will be approximately "now", which falls inside
  // the interval.  This lets us verify that the Created/Modified toggle
  // actually filters against different stat timestamps.
  const now = new Date();
  const twoYearsAgo = new Date(now.getFullYear() - 2, now.getMonth(), now.getDate());
  const twoYearsAgoSec = Math.floor(twoYearsAgo.getTime() / 1000);

  const staleMtimeFile = "stale-mtime.txt";
  fs.writeFileSync(path.join(hostSortDir, staleMtimeFile), "btime≈now mtime=old\n");
  fs.utimesSync(path.join(hostSortDir, staleMtimeFile), twoYearsAgoSec, twoYearsAgoSec);

  const baselineFiles = ["aardvark.txt", "zebra.txt", "baby.txt"];

  // ── Modified interval ─────────────────────────────────────────────────
  //  stale-mtime.txt has mtime = two years ago, outside the current year
  //  ±1 range, so it must NOT be found.  The baseline files have mtimes set
  //  to ~now by local.mjs, so they must be found.
  const modFound = await runDatetimeIntervalSearch(browser, observer, "modified", sortDir, null);
  if (modFound < baselineFiles.length) {
    throw new Error(
      `Datetime interval search (modified) expected ${baselineFiles.length} files but found ${modFound} in ${sortDir}`
    );
  }
  const modStale = await runDatetimeIntervalSearch(browser, observer, "modified", sortDir, staleMtimeFile);
  if (modStale) {
    throw new Error(
      `'${staleMtimeFile}' (mtime = two years ago) was unexpectedly found by modified-interval search (should be outside current year ±1)`
    );
  }

  // ── Created interval ──────────────────────────────────────────────────
  //  btime ≈ now, which is inside the current year ±1 range.  If the
  //  filesystem supports birth-time the file must appear; if not it won't.
  const createdStale = await runDatetimeIntervalSearch(browser, observer, "created", sortDir, staleMtimeFile);
  // btime is unchangeable — if the FS reports it, the file appears.
  if (createdStale) {
    // Extra confidence: the baseline files should also be present if
    // btime works.  Re-run to check.
    const createdBaseline = await runDatetimeIntervalSearch(browser, observer, "created", sortDir, null);
    if (createdBaseline < baselineFiles.length) {
        throw new Error(
          `Datetime interval search (created) expected ${baselineFiles.length} files but found ${createdBaseline} in ${sortDir}`
        );
      }
    }
  }

// ─── Size search flow ────────────────────────────────────────────────────────

/**
 * Open search in sortDir, switch to Extra tab, enable size filter with the
 * given op/value/max (all in bytes), run the search, mount results, and
 * return whether `checkFile` is found.
 */
async function runSizeSearch(browser, observer, sortDir, sizeOp, value, max, checkFile) {
  observer.navigatePanelToPath("left", sortDir);
  await observer.waitForPanelPath("left", sortDir);
  await observer.waitForPanelNotShadow("left");
  await reloadBrowserApp(browser, "fbro-a");

  const maskInput = await openSearch(browser);
  await maskInput.setValue("*");

  const extraTab = await browser.$('.search-modal__tab:nth-child(3)');
  await extraTab.waitForDisplayed({ timeout: 5_000 });
  await extraTab.click();

  // Enable file size filter
  const sizeCheckbox = await browser.$('.search-modal__section:first-child input[type="checkbox"]');
  await sizeCheckbox.waitForDisplayed({ timeout: 5_000 });
  await sizeCheckbox.click();

  // Select size operator from the dropdown
  const opSelect = await browser.$('.search-modal__extra-select');
  await opSelect.waitForDisplayed({ timeout: 5_000 });
  await opSelect.selectByAttribute("value", sizeOp);

  // Cycle unit to Bytes (default KiB → MiB → GiB → B = 3 clicks)
  const unitBtn = await browser.$('.search-modal__unit-badge');
  await unitBtn.waitForDisplayed({ timeout: 3_000 });
  await unitBtn.click();
  await unitBtn.click();
  await unitBtn.click();

  // Type size value
  const sizeInput = await browser.$('input[aria-label="File size value"]');
  await sizeInput.waitForDisplayed({ timeout: 5_000 });
  await sizeInput.setValue(value);

  // Type size max for interval mode
  if (sizeOp === "interval" && max != null) {
    const maxInput = await browser.$('input[aria-label="File size max"]');
    await maxInput.waitForDisplayed({ timeout: 5_000 });
    await maxInput.setValue(max);
  }

  await clickCommand(browser, "search.start");
  await clickCommand(browser, "search.to-results");
  await observer.waitForSearchResultsMounted("left");

  if (checkFile) {
    try {
      await observer.waitForEntry("left", checkFile, "file", 10_000);
      return true;
    } catch {
      return false;
    }
  }
  return false;
}

export async function runSizeSearchFlow(browser, observer) {
  const sortDir = path.posix.join(backendPath("a", "src"), "sort-test");

  // Files: aardvark.txt = 500 B, zebra.txt = 10 B, baby.txt = 100 B

  // ── "Bigger than" 200 Bytes ───────────────────────────────────────────
  const gtFound = await runSizeSearch(browser, observer, sortDir, "gt", "200", null, "aardvark.txt");
  if (!gtFound) {
    throw new Error("Size search 'Bigger than 200 B' did not find aardvark.txt (500 B)");
  }
  await observer.waitForNoEntry("left", "zebra.txt");
  await observer.waitForNoEntry("left", "baby.txt");

  // ── "In range" 50–200 Bytes ───────────────────────────────────────────
  const rangeFound = await runSizeSearch(browser, observer, sortDir, "interval", "50", "200", "baby.txt");
  if (!rangeFound) {
    throw new Error("Size search 'In range 50–200 B' did not find baby.txt (100 B)");
  }
  await observer.waitForNoEntry("left", "aardvark.txt");
  await observer.waitForNoEntry("left", "zebra.txt");
}
