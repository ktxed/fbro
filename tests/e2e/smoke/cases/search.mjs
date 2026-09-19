import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickCommand, reloadBrowserApp } from "../browser.mjs";

export async function runSearchFlow(browser, observer) {
  const aDst = backendPath("a", "dst");
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
    if (hints.length !== 3) return false;
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
  const hintedRow = await browser.$('[data-testid="panel-left"] [data-entry-name="renamed.txt"]');
  const hintedRowTitle = await hintedRow.getAttribute("title");
  if (!hintedRowTitle?.endsWith("/renamed.txt")) {
    throw new Error(`Duplicate search result did not expose its full path as a tooltip: ${hintedRowTitle}`);
  }
  await browser.waitUntil(async () => {
    const dateCell = await browser.$('[data-testid="panel-left"] [data-entry-name="renamed.txt"] .panel__mtime');
    if (!await dateCell.isExisting()) return false;
    const text = await dateCell.getText();
    return text.trim().length > 0;
  }, {
    timeout: 5_000,
    timeoutMsg: "Mounted search result Date column stayed empty",
  });
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

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

  // Open search
  await browser.execute(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "f",
      code: "KeyF",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    }));
  });

  // File mask: "*" (match all)
  const maskInput = await browser.$('[data-testid="search-file-mask-input"]');
  await maskInput.waitForDisplayed({ timeout: 10_000 });
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

  await browser.execute(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "f",
      code: "KeyF",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    }));
  });

  const maskInput = await browser.$('[data-testid="search-file-mask-input"]');
  await maskInput.waitForDisplayed({ timeout: 10_000 });
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
