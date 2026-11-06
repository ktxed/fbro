// Each scene starts from a known state (prepare), stages one view, and calls
// shot(). Keep scenes independent so a single one can be re-run:
//   npm run docs:screens -- first-launch
import fs from "node:fs";
import { execFileSync } from "node:child_process";
import { composeFiles, hostPath } from "../smoke/config.mjs";
import { clickContextMenuItem, clickEntry, clickSource, goToPanelPath, pressPanelKey, setTextInputValue, terminalToolbarBox, waitForMainWindow } from "../smoke/browser.mjs";
import { sleep, waitFor } from "../smoke/wait.mjs";
import { demo } from "./fixture.mjs";
import { shot } from "./shot.mjs";

function isGrid(browser, panelId) {
  return browser.execute((id) =>
    !!document.querySelector(`[data-testid="panel-${id}"] .panel__list--grid`), panelId);
}

async function setViewMode(browser, observer, panelId, mode) {
  if (await isGrid(browser, panelId) === (mode === "grid")) return;
  await focusPanel(browser, panelId);
  await browser.keys("g");
  await waitFor(async () => await isGrid(browser, panelId) === (mode === "grid"), { label: `${panelId} ${mode} view` });
}

const debug = (...args) => { if (process.env.DOCS_DEBUG) console.log("[docs:debug]", ...args); };
const panelInfo = (observer, id) => {
  const p = observer.state.panels[id];
  return p && { path: p.path, filter: p.filter, filterModeActive: p.filterModeActive, viewMode: p.viewMode, sel: p.selection?.length };
};

async function navigate(browser, observer, panelId, target, mode = "list") {
  debug("navigate", panelId, target, "before", panelInfo(observer, panelId), "panels", Object.keys(observer.state.panels));
  // Navigate through the window's own Go To Path, like the smoke cases do.
  await goToPanelPath(browser, observer, panelId, target);
  await setViewMode(browser, observer, panelId, mode);
  observer.sendIntent("ClearSelection", panelId);
  await waitFor(async () => await selectedCount(browser, panelId) === 0, { label: `${panelId} selection cleared` });
  debug("navigate done", panelId, panelInfo(observer, panelId));
}

// The observer doesn't receive selection patches; read the rendered rows instead.
function selectedCount(browser, panelId) {
  return browser.execute((id) => document.querySelectorAll(
    `[data-testid="panel-${id}"] .panel__row--selected, [data-testid="panel-${id}"] .panel__card--selected`).length, panelId);
}

async function focusPanel(browser, panelId) {
  await browser.execute((id) => document.querySelector(`[data-testid="panel-${id}"]`)?.focus(), panelId);
  await waitFor(() => browser.execute((id) =>
    document.querySelector(`[data-testid="panel-${id}"]`)?.classList.contains("panel--focused"), panelId),
  { label: `${panelId} focused` });
}

async function select(browser, observer, panelId, names) {
  for (const name of names) {
    await clickEntry(browser, panelId, name);
    await browser.keys("Space");
  }
  await waitFor(async () => await selectedCount(browser, panelId) === names.length,
    { label: `${panelId} ${names.length} selected` });
}

async function closeSettings(browser) {
  for (let i = 0; i < 4; i++) {
    if (!await browser.$('[data-testid="settings-tab-general"]').isExisting()) return;
    await browser.keys("Escape");
    await sleep(300);
  }
}

async function closeModals(browser) {
  for (let i = 0; i < 4; i++) {
    if (!await browser.execute(() => !!document.querySelector('[role="dialog"]'))) return;
    await browser.keys("Escape");
    await sleep(300);
  }
}

async function cancelJob(browser) {
  await browser.$('[data-command-id="job.cancel"]').then((el) => el.click());
  const confirm = await browser.$('[data-command-id="job.cancel.confirm"]');
  if (await confirm.isExisting()) await confirm.click();
  await sleep(500);
  await closeModals(browser);
}

async function broOption(browser, broId) {
  const option = await browser.$(`[data-testid="bro-option"][data-bro-id="${broId}"]`);
  await option.waitForClickable({ timeout: 20_000 });
  await option.click();
}

/** Default layout: website on the left, backup on the right, both in list view. */
async function prepare(browser, observer) {
  await closeSettings(browser);
  await navigate(browser, observer, "right", demo.backup);
  await navigate(browser, observer, "left", demo.website);
  await focusPanel(browser, "left");
}

export const scenes = {
  // get-started/first-launch, index
  "first-launch": async (browser, observer) => {
    await prepare(browser, observer);
    await select(browser, observer, "left", ["index.html", "styles.css"]);
    await clickEntry(browser, "left", "README.md");
    await shot(browser, "first-launch");
  },

  // using/selection
  "select-files": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.photos);
    await focusPanel(browser, "left");
    await clickEntry(browser, "left", "IMG_0412.jpg");
    await browser.execute(() => document.activeElement.dispatchEvent(
      new KeyboardEvent("keydown", { key: "+", code: "NumpadAdd", bubbles: true, cancelable: true })));
    await browser.$('[data-testid="selection-mask-dialog"]').then((el) => el.waitForDisplayed());
    await shot(browser, "select-files");
    await browser.keys("Escape");
    await browser.$('[data-testid="selection-mask-dialog"]').then((el) => el.waitForExist({ reverse: true }));
  },

  // using/views
  "grid-view": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.photos, "grid");
    await focusPanel(browser, "left");
    await clickEntry(browser, "left", "IMG_0418.jpg");
    await waitFor(() => browser.execute(() =>
      document.querySelectorAll('[data-testid="panel-left"] img').length >= 6), { timeout: 30_000, label: "thumbnails" });
    await shot(browser, "grid-view");
    await setViewMode(browser, observer, "left", "list");
  },

  // files/operations
  "copy-dialog": async (browser, observer) => {
    await prepare(browser, observer);
    await select(browser, observer, "left", ["README.md", "index.html", "styles.css"]);
    await pressPanelKey(browser, "left", "F5");
    await browser.$('[data-testid="operation-dialog"]').then((el) => el.waitForDisplayed());
    await shot(browser, "copy-dialog");
    await browser.keys("Escape");
    await browser.$('[data-testid="operation-dialog"]').then((el) => el.waitForExist({ reverse: true }));
  },

  // viewing/viewer: the viewer window's page, loaded into the main WebView
  // the same way tests/e2e/smoke/cases/viewer.mjs does.
  "viewer-markdown": async (browser, observer) => {
    await prepare(browser, observer);
    const mainUrl = (await browser.getUrl()).split("?")[0];
    const file = `${demo.website}/README.md`;
    const params = new URLSearchParams({ viewer: "1", path: file, source: observer.state.panels.left.sourceId,
      name: "README.md", label: "main", panel: "left", local: "1",
      size: String(fs.statSync(hostPath(file)).size), fsport: String(observer.state.fileServerPort) });
    try {
      await browser.url(`${mainUrl}?${params}`);
      await waitFor(() => browser.execute(() => !!document.querySelector(".viewer__filename")), { label: "viewer loaded" });
      await browser.keys("4");
      await waitFor(() => browser.execute(() =>
        !!document.querySelector(".viewer__markdown svg") && !document.querySelector(".viewer__markdown code.language-mermaid")),
      { timeout: 20_000, label: "markdown with mermaid" });
      await shot(browser, "viewer-markdown");
    } finally {
      await browser.url(mainUrl);
      await waitForMainWindow(browser, "docs: back to main");
    }
  },

  // using/sorting-filtering
  "filter": async (browser, observer) => {
    await prepare(browser, observer);
    await focusPanel(browser, "left");
    await browser.keys("f");
    await browser.$('[data-testid="panel-left"] .panel__filter input, [data-testid="panel-left"] input').then((el) => el.waitForDisplayed());
    await browser.keys(["m", "d"]);
    await waitFor(() => browser.execute(() => document.querySelectorAll('[data-testid="panel-left"] [data-testid="panel-entry"]').length <= 3),
      { label: "filtered listing" });
    await shot(browser, "filter");
    await browser.keys("Escape");
    await browser.keys("Escape");
  },

  "sort-selector": async (browser, observer) => {
    await prepare(browser, observer);
    await focusPanel(browser, "left");
    await browser.keys(["Shift", "s"]);
    await browser.$('[data-testid="panel-sort-selector"]').then((el) => el.waitForDisplayed());
    for (const key of ["ArrowRight", "ArrowRight", "ArrowDown"]) {
      await browser.keys(key);
      await sleep(200);
    }
    await shot(browser, "sort-selector");
    await browser.keys("Escape");
  },

  // using/bookmarks-history
  "bookmarks": async (browser, observer) => {
    await prepare(browser, observer);
    for (const folder of [demo.photos, demo.backup]) {
      await navigate(browser, observer, "left", folder);
      await focusPanel(browser, "left");
      await browser.keys(["Control", "d"]);
      await browser.$('[data-testid="bookmarks-popover"]').then((el) => el.waitForDisplayed());
      await browser.$('[data-testid="bookmark-current-toggle"]').then((el) => el.click());
      await browser.keys("Escape");
    }
    await navigate(browser, observer, "left", demo.website);
    await focusPanel(browser, "left");
    await browser.keys(["Control", "d"]);
    await browser.$('[data-testid="bookmarks-popover"]').then((el) => el.waitForDisplayed());
    await shot(browser, "bookmarks");
    await browser.keys("Escape");
  },

  // using/command-palette
  "command-palette": async (browser, observer) => {
    await prepare(browser, observer);
    await browser.keys(["Control", "Shift", "p"]);
    await browser.$('[role="dialog"] input').then((el) => el.waitForDisplayed());
    await browser.keys(["g", "r", "i", "d"]);
    await sleep(500);
    await shot(browser, "command-palette");
    await browser.keys("Escape");
  },

  "cheat-sheet": async (browser, observer) => {
    await prepare(browser, observer);
    await browser.keys("?");
    await browser.$('[data-testid="cheat-sheet-body"]').then((el) => el.waitForDisplayed());
    await shot(browser, "cheat-sheet");
    await browser.keys("Escape");
  },

  "context-menu": async (browser, observer) => {
    await prepare(browser, observer);
    await select(browser, observer, "left", ["index.html", "styles.css"]);
    await browser.keys("c");
    await browser.$(".context-menu").then((el) => el.waitForDisplayed());
    await shot(browser, "context-menu");
    await browser.keys("Escape");
  },

  // files/inline-choices
  "inline-choices": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.backup);
    await focusPanel(browser, "left");
    await clickEntry(browser, "left", "photos-2026.zip");
    await browser.$('[data-testid="file-inline-bubble"]').then((el) => el.waitForDisplayed());
    await shot(browser, "inline-choices");
  },

  // bros/overview
  "bro-list": async (browser, observer) => {
    await prepare(browser, observer);
    await select(browser, observer, "left", ["docs", "src"]);
    await browser.keys("F1");
    await browser.$('[data-testid="bro-option"]').then((el) => el.waitForDisplayed());
    await shot(browser, "bro-list");
    await browser.keys("Escape");
  },

  // viewing/quick-preview
  "quick-preview": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.photos);
    await focusPanel(browser, "left");
    await clickEntry(browser, "left", "IMG_0418.jpg");
    await browser.keys(["Control", "F3"]);
    await browser.$('[data-testid^="preview-"]').then((el) => el.waitForDisplayed());
    await waitFor(() => browser.execute(() => !!document.querySelector('[data-testid^="preview-"] img')), { label: "preview image" });
    await shot(browser, "quick-preview");
    await browser.keys(["Control", "F3"]);
  },

  // viewing/terminal
  "terminal": async (browser, observer) => {
    await prepare(browser, observer);
    await focusPanel(browser, "left");
    await browser.keys("t");
    await browser.$(".xterm").then((el) => el.waitForDisplayed());
    await sleep(1500);
    await browser.keys(["l", "s", " ", "-", "l", "a", "Enter"]);
    await sleep(1500);
    // The action toolbar fades out once the pointer leaves its corner, and
    // settle() parks the pointer before every capture, so park it just inside
    // the toolbar's approach region instead of the usual window corner: the
    // guide picture then shows the buttons beside the output, and the pointer
    // stays there for the close click below.
    const toolbarBox = await terminalToolbarBox(browser, '[data-testid="panel-left"]');
    await shot(browser, "terminal", {
      pointerAt: { x: toolbarBox.left - 20, y: toolbarBox.top + 10 },
    });
    const close = await browser.$(".panel-terminal__close");
    await close.waitForClickable({ timeout: 5_000 });
    await close.click();
    await waitFor(() => browser.execute(() => !document.querySelector(".xterm")), { label: "terminal closed" });
  },

  // customize/settings
  "settings-general": async (browser, observer) => {
    await prepare(browser, observer);
    await browser.keys(["Control", ","]);
    await browser.$('[data-testid="settings-tab-general"]').then((el) => el.waitForDisplayed());
    // Start at the Files section: the Shell command section above it shows the
    // host's install path.
    await browser.execute(() => {
      const heading = [...document.querySelectorAll('[role="dialog"] *')].find((el) => el.children.length === 0 && el.textContent.trim().toUpperCase() === "FILES");
      heading?.scrollIntoView({ block: "start" });
    });
    await sleep(300);
    await shot(browser, "settings-general");
    await browser.keys("Escape");
  },

  "settings-shortcuts": async (browser, observer) => {
    await prepare(browser, observer);
    await browser.keys(["Control", ","]);
    await browser.$('[data-testid="settings-tab-general"]').then((el) => el.waitForDisplayed());
    await browser.$('[data-testid="settings-tab-keyboard"]').then((el) => el.click());
    await setTextInputValue(browser, '[data-testid="keyboard-search"]', "copy");
    await sleep(500);
    await shot(browser, "settings-shortcuts");
    await browser.keys("Escape");
  },

  // customize/color-schemes
  "color-scheme-editor": async (browser, observer) => {
    await prepare(browser, observer);
    await browser.keys(["Control", ","]);
    await browser.$('[data-testid="settings-tab-general"]').then((el) => el.waitForDisplayed());
    await browser.$('[data-testid="settings-tab-display"]').then((el) => el.click());
    await browser.$('[data-testid="customize-color-scheme"]').then((el) => el.click());
    await browser.$('[data-testid="color-scheme-editor"]').then((el) => el.waitForDisplayed());
    await browser.$('[data-testid="color-editor-duplicate"]').then((el) => el.click());
    await setTextInputValue(browser, '[data-testid="color-editor-duplicate-name"]', "Midnight");
    await browser.$('[data-testid="color-editor-duplicate-confirm"]').then((el) => el.click());
    await sleep(800);
    await shot(browser, "color-scheme-editor");
    await closeSettings(browser);
  },

  // git/repositories
  "git-listing": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.repo);
    await focusPanel(browser, "left");
    await waitFor(() => browser.execute(() => !!document.querySelector('[data-testid="panel-left"] [class*="git"]')), { label: "git status markers" });
    await clickEntry(browser, "left", "README.md");
    await sleep(800);
    await shot(browser, "git-listing");
  },

  "git-changes": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.repo);
    await focusPanel(browser, "left");
    await browser.keys(["Shift", "g"]);
    await browser.$('[data-testid="git-tab-changes"]').then((el) => el.waitForDisplayed());
    await browser.$('[data-testid="git-change-row"]').then((el) => el.waitForDisplayed());
    await browser.$('textarea').then((el) => el.setValue("Make watering depend on rain"));
    await shot(browser, "git-changes");
    await browser.keys("Escape");
    await closeModals(browser);
  },

  "git-log": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.repo);
    await focusPanel(browser, "left");
    await browser.keys(["Shift", "g"]);
    await browser.$('[data-testid="git-tab-log"]').then((el) => el.click());
    await browser.$('[data-testid="git-log-row"]').then((el) => el.waitForDisplayed());
    await browser.$('[data-testid="git-log-row"]').then((el) => el.doubleClick());
    await browser.$('[data-testid="git-log-commit-show"] .git-modal__change-actions').then((el) => el.waitForDisplayed());
    await sleep(500);
    await shot(browser, "git-log");
    await closeModals(browser);
  },

  // files/operations
  "conflict-prompt": async (browser, observer) => {
    await prepare(browser, observer);
    await select(browser, observer, "left", ["README.md"]);
    await pressPanelKey(browser, "left", "F5");
    await browser.$('[data-testid="operation-dialog"]').then((el) => el.waitForDisplayed());
    await browser.$('[data-testid="file-conflict-policy"]').then((el) => el.selectByAttribute("value", "ask"));
    await browser.$('[data-command-id="file.copy.confirm"]').then((el) => el.click());
    await waitFor(() => browser.execute(() => document.body.textContent.includes("File already exists:")), { label: "conflict prompt" });
    await shot(browser, "conflict-prompt");
    await cancelJob(browser);
  },

  // files/jobs
  "jobs-dialog": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.big);
    await focusPanel(browser, "left");
    await clickEntry(browser, "left", "footage-2026.mov");
    await pressPanelKey(browser, "left", "F5");
    await browser.$('[data-testid="operation-dialog"]').then((el) => el.waitForDisplayed());
    // Start paused is on by default; this capture wants a copy that is running.
    await browser.$('[data-testid="file-operation-start-paused"]').then((el) => el.click());
    await browser.$('[data-command-id="file.copy.background"]').then((el) => el.click());
    await waitFor(() => browser.execute(() => !document.querySelector('[data-testid="operation-dialog"]')), { label: "sent to background" });
    await focusPanel(browser, "left");
    await browser.keys("j");
    await browser.$('[data-testid="job-row"]').then((el) => el.waitForDisplayed());
    // Pause so the frame is stable: mid-copy, with matching bar and percentage.
    await waitFor(() => browser.execute(() => /[1-9]\d*%/.test(document.querySelector('[data-testid="job-row"]')?.textContent ?? "")), { label: "copy progress" });
    await browser.execute(() => [...document.querySelectorAll('[data-testid="job-row"] button')].find((b) => /pause/i.test(b.textContent))?.click());
    await waitFor(() => browser.execute(() => /paused/i.test(document.querySelector('[data-testid="job-row"]')?.textContent ?? "")), { label: "paused" });
    await sleep(1200);
    await shot(browser, "jobs-dialog");
    await browser.$('[data-testid="job-cancel"]').then((el) => el.click());
    await sleep(800);
    await closeModals(browser);
    fs.rmSync(hostPath(`${demo.backup}/footage-2026.mov`), { force: true });
  },

  // bros/duplicates
  "fedup-results": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", "/work/a");
    await observer.waitForEntry("left", "demo", "directory");
    observer.sendIntent("AddToGlobalSelection", "left", { panelId: "left", targetPath: demo.root, name: "Lighthouse demo" });
    await observer.waitForGlobalSelection("Lighthouse demo", 1, 20_000);
    observer.sendIntent("NavigateToSource", "left", { sourceId: "global-selections" });
    await waitFor(() => observer.state.panels.left?.sourceId === "global-selections" && observer.state.panels.left.status === "ready",
      { label: "global selections" });
    const index = observer.state.panels.left.entries.findIndex((e) => e.name === "Lighthouse demo");
    observer.sendIntent("SetCursor", "left", { index });
    await waitFor(() => observer.state.panels.left.cursor === index, { label: "cursor" });
    await focusPanel(browser, "left");
    await browser.keys("F1");
    await broOption(browser, "builtin:fedup");
    await browser.$('[data-testid="bro-launch-button"]').then((el) => el.click());
    await waitFor(() => browser.execute(() => document.querySelector('[role="dialog"] .modal__header')?.textContent.includes("Completed")
      || /duplicate/i.test(document.querySelector('[role="dialog"]')?.textContent ?? "") && !!document.querySelector('[data-command-id="job.close"]')),
    { timeout: 60_000, label: "fedup results" });
    await sleep(800);
    await shot(browser, "fedup-results");
    await closeModals(browser);
  },

  // bros/hashes
  "hash-results": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "left", demo.photos);
    await focusPanel(browser, "left");
    await browser.keys(["Control", "a"]);
    await waitFor(async () => await selectedCount(browser, "left") >= 10, { label: "photos selected" });
    await browser.keys("F1");
    await broOption(browser, "builtin:generate-hashes");
    await browser.$('[data-testid="bro-launch-button"]').then((el) => el.click());
    await waitFor(() => browser.execute(() => document.querySelector('[data-testid="hash-generate-dialog"] .modal__header')?.textContent.includes("Completed")),
      { timeout: 60_000, label: "hash generation" });
    await sleep(800);
    await shot(browser, "hash-results");
    await closeModals(browser);
    for (const f of fs.readdirSync(hostPath(demo.photos))) if (f.endsWith(".b3")) fs.rmSync(hostPath(`${demo.photos}/${f}`));
  },

  // viewing/compare: the diff window's page, loaded into the main WebView the
  // way tests/e2e/smoke/cases/diff.mjs does. The native window opens in the
  // backend container and its compare session is read from the backend log.
  "diff-window": async (browser, observer) => {
    await prepare(browser, observer);
    await navigate(browser, observer, "right", demo.backupOctober);
    await clickEntry(browser, "right", "CHANGELOG.md");
    await clickEntry(browser, "left", "CHANGELOG.md");
    const mainUrl = (await browser.getUrl()).split("?")[0];
    const sessionIds = () => [...execFileSync("docker", ["compose", ...composeFiles.flatMap((file) => ["-f", file]),
      "logs", "--no-color", "fbro-a"], { encoding: "utf8" })
      .matchAll(/GetCompareDiff scope=null args=\{"requestId":"([^"]+)"\}/g)].map((match) => match[1]);
    const previous = new Set(sessionIds());
    observer.sendIntent("CompareFiles", undefined, { panelId: "left" });
    let requestId;
    await waitFor(() => (requestId = sessionIds().find((id) => !previous.has(id))), { timeout: 20_000, label: "compare session" });
    try {
      await browser.url(`${mainUrl}?diff=1&request=${encodeURIComponent(requestId)}`);
      await browser.$('[data-testid="diff-viewer"]').then((el) => el.waitForDisplayed({ timeout: 15_000 }));
      await sleep(800);
      await shot(browser, "diff-window");
    } finally {
      observer.sendIntent("CloseCompareWindow", undefined, { requestId, label: `diff-${requestId}` });
      await browser.url(mainUrl);
      await waitForMainWindow(browser, "docs: back to main");
    }
  },

  // viewing/editor
  "editor-tab": async (browser, observer) => {
    await prepare(browser, observer);
    await clickEntry(browser, "left", "README.md");
    await browser.keys("c");
    await clickContextMenuItem(browser, "Edit in Editor Tab");
    await waitFor(() => browser.execute(() => !!document.querySelector(".panel-editor .cm-content")), { label: "editor tab" });
    await browser.keys(["Control", "F3"]);
    await waitFor(() => browser.execute(() =>
      !!document.querySelector('[data-testid="preview-right"] .viewer__markdown svg')), { timeout: 20_000, label: "rendered preview" });
    await shot(browser, "editor-tab");
    await browser.keys(["Control", "F3"]);
    await browser.keys(["Control", "w"]);
    await waitFor(() => browser.execute(() => !document.querySelector(".panel-editor")), { label: "editor closed" });
  },

  // remote/pairing: both windows show the confirmation; instance A is captured.
  "pairing": async (browser, observer, peer) => {
    await pairInstances(browser, observer, peer, { capture: true });
  },

  // remote/pairing
  "sharing-settings": async (browser, observer, peer) => {
    await pairInstances(browser, observer, peer);
    await openSharingSettings(browser);
    await sleep(500);
    await shot(browser, "sharing-settings");
    await closeSettings(browser);
  },

  // remote/browse-transfer: B's shared photos, with thumbnails fetched from B.
  "remote-browse": async (browser, observer, peer) => {
    const source = await pairInstances(browser, observer, peer);
    await showRemotePhotos(browser, observer, source);
    await setViewMode(browser, observer, "left", "grid");
    await clickEntry(browser, "left", "IMG_0418.jpg");
    await waitFor(() => browser.execute(() =>
      document.querySelectorAll('[data-testid="panel-left"] img').length >= 6), { timeout: 40_000, label: "remote thumbnails" });
    await shot(browser, "remote-browse");
    await setViewMode(browser, observer, "left", "list");
  },

  // remote/sync: B's photos against a local copy where one photo differs.
  "sync-results": async (browser, observer, peer) => {
    const source = await pairInstances(browser, observer, peer);
    await showRemotePhotos(browser, observer, source);
    await setViewMode(browser, observer, "left", "list");
    await goToPanelPath(browser, observer, "right", demo.photosCopy);
    await focusPanel(browser, "left");
    await browser.keys("F1");
    await broOption(browser, "builtin:sync-bro");
    // Sync Bro has no parameters, so choosing it starts the comparison.
    const launch = await browser.$('[data-testid="bro-launch-button"]');
    if (await launch.isExisting()) await launch.click();
    await waitFor(() => browser.execute(() => document.querySelector('[role="dialog"] .modal__header')?.textContent.includes("Completed")),
      { timeout: 60_000, label: "sync results" });
    await sleep(800);
    await shot(browser, "sync-results");
    await closeModals(browser);
  },
};

export const needsPeer = new Set(["pairing", "sharing-settings", "remote-browse", "sync-results"]);

const remote = { paired: null };

async function openSharingSettings(browser) {
  await focusPanel(browser, "left");
  await browser.keys(["Control", ","]);
  await browser.$('[data-testid="settings-tab-general"]').then((el) => el.waitForDisplayed());
  await browser.$('[data-testid="settings-tab-sharing"]').then((el) => el.click());
}

async function shareFolder(browser, folder) {
  await openSharingSettings(browser);
  await setTextInputValue(browser, '[data-testid="sharing-folder-path"]', folder);
  await browser.$('[data-command-id="sharing.folder.add"]').then((el) => el.click());
  await sleep(500);
  await closeSettings(browser);
}

/** Pairs instance A with B once per run; returns A's remote source id for B's photos. */
async function pairInstances(browser, observer, peer, { capture = false } = {}) {
  if (remote.paired) {
    if (capture) throw new Error("pairing must run before the other remote scenes");
    return remote.paired;
  }
  await prepare(browser, observer);
  const { browser: browserB, observer: observerB } = peer;
  for (const o of [observer, observerB]) {
    o.sendIntent("SetGeneralOption", undefined, { key: "osNotificationsEnabled", value: false });
  }

  // B shares its photos; A shares the website folder and keeps a local photo copy with one change.
  fs.rmSync(hostPath(demo.bPhotos), { recursive: true, force: true });
  fs.cpSync(hostPath(demo.photos), hostPath(demo.bPhotos), { recursive: true, preserveTimestamps: true });
  fs.rmSync(hostPath(demo.photosCopy), { recursive: true, force: true });
  fs.cpSync(hostPath(demo.photos), hostPath(demo.photosCopy), { recursive: true, preserveTimestamps: true });
  const changed = hostPath(`${demo.photosCopy}/IMG_0431.jpg`);
  const { atime, mtime } = fs.statSync(changed);
  fs.appendFileSync(changed, Buffer.from([0]));
  fs.utimesSync(changed, atime, mtime);
  observer.sendIntent("EnableSharing", undefined, { enabled: true });
  observerB.sendIntent("EnableSharing", undefined, { enabled: true });
  await waitFor(() => observer.state.shareConfig.enabled && observerB.state.shareConfig.enabled, { label: "sharing enabled" });
  await shareFolder(browserB, demo.bPhotos);
  await shareFolder(browser, demo.website);
  const target = await waitFor(() => observerB.state.shareConfig.sharedRoots.find((t) => t.path === demo.bPhotos), { label: "B share" });

  await openSharingSettings(browser);
  await browser.$('[data-testid="direct-pair-address"]').then((el) => el.setValue("fbro-b:7880"));
  await browser.$('[data-command-id="sharing.pair-direct"]').then((el) => el.click());
  await browser.$('[data-testid="pairing-confirmation"]').then((el) => el.waitForDisplayed({ timeout: 20_000 }));
  await browserB.$('[data-testid="pairing-confirmation"]').then((el) => el.waitForDisplayed({ timeout: 20_000 }));
  if (capture) {
    await sleep(500);
    await shot(browser, "pairing");
  }
  await Promise.all([browser, browserB].map((b) => b.$('[data-command-id="pairing.confirm"]').then((el) => el.click())));
  await waitFor(() => observer.state.shareConfig.pairedPeers?.length > 0, { timeout: 20_000, label: "paired" });
  await closeSettings(browser);
  await closeSettings(browserB);

  const fingerprint = observer.state.shareConfig.pairedPeers[0].fingerprint;
  observer.sendIntent("RetryLastPeer", undefined, { fingerprint });
  const source = await waitFor(() => Object.values(observer.state.sources)
    .find((src) => src.kind === "remote" && src.id.endsWith(`:${target.id}`)), { timeout: 30_000, label: "remote source" });
  remote.paired = source.id;
  return source.id;
}

async function showRemotePhotos(browser, observer, sourceId) {
  await focusPanel(browser, "left");
  await clickSource(browser, "left", sourceId);
  await waitFor(() => browser.execute(() =>
    !!document.querySelector('[data-testid="panel-left"] [data-entry-name="IMG_0418.jpg"]')), { timeout: 30_000, label: "remote photos listed" });
  await focusPanel(browser, "left");
}
