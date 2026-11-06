import assert from "node:assert/strict";
import { pressPanelKey, setTextInputValue } from "../browser.mjs";
import { sleep, waitFor } from "../wait.mjs";

const SLOT = "left";
const countButton = `[data-testid="panel-tabs-count-${SLOT}"]`;
const widget = '[data-testid="panel-tab-list"]';
const input = '[data-testid="panel-tab-list-input"]';
const rows = `${widget} [data-testid^="panel-tab-list-row-"]`;
const rowOf = (panelId) => `[data-testid="panel-tab-list-row-${panelId}"]`;
const panelBox = (panelId) => `[data-testid="panel-${panelId}"]`;

const parentOf = (path) => {
  const trimmed = path.replace(/\/+$/, "");
  const cut = trimmed.lastIndexOf("/");
  return cut <= 0 ? "/" : trimmed.slice(0, cut);
};

const leafOf = (path) => {
  const trimmed = path.replace(/\/+$/, "");
  const cut = trimmed.lastIndexOf("/");
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
};

async function snapshot(observer) {
  const start = observer.messages.length;
  observer.sendIntent("RequestStateSnapshot");
  await waitFor(() => observer.messages.slice(start).some((m) => m.type === "StateSnapshot"),
    { label: "tab list authoritative snapshot" });
  return observer.state.slots[SLOT];
}

const activeTab = async (observer) => (await snapshot(observer)).activePanelId;
const exists = async (browser, selector) => (await browser.$(selector)).isExisting();
const focusedIs = (browser, panelId) => browser.execute((selector) =>
  document.activeElement?.closest(selector) !== null, panelBox(panelId));
const focusIsOpener = (browser) => browser.execute(() =>
  document.activeElement?.dataset.tabListOpener === "true");

/** Park focus on the slot's current listing and mark it as the expected opener. */
async function markOpener(browser, observer) {
  const panelId = await activeTab(observer);
  await browser.execute((selector) => {
    const panel = document.querySelector(selector);
    panel.focus();
    panel.dataset.tabListOpener = "true";
  }, panelBox(panelId));
  await waitFor(() => observer.state.focus === panelId, { label: `panel ${panelId} active` });
  return panelId;
}

const highlighted = (browser) => browser.execute((rows) => {
  const selected = document.querySelector(`${rows}[data-selected="true"]`);
  return selected ? selected.dataset.testid.replace("panel-tab-list-row-", "") : null;
}, rows);

const renderedRows = (browser) => browser.execute((rows) => Array.from(
  document.querySelectorAll(rows), (element) => element.dataset.testid.replace("panel-tab-list-row-", ""),
), rows);

/** Type one character at a time, like the shortcut case does, so the host
 *  keyboard layout cannot turn a letter into something else. */
async function type(browser, text) {
  for (const character of text) {
    await browser.action("key").down(character).up(character).perform();
  }
}

/** Roam the highlight to one row, so no assertion depends on row order. */
async function highlightRow(browser, panelId) {
  await browser.keys("Home");
  for (let step = 0; step <= 40; step++) {
    if (await highlighted(browser) === panelId) return;
    await browser.keys("ArrowDown");
  }
  throw new Error(`highlight never reached row ${panelId}`);
}

export async function runTabListFlow(browser, observer) {
  const original = await snapshot(observer);
  const originalIds = [...original.tabPanelIds];
  const originalActive = original.activePanelId;
  // Clones copy the active tab, so the fixture paths hang off that path.
  const leftPath = observer.state.panels[originalActive]?.path ?? "";
  const created = [];
  const otherSlotId = Object.keys(observer.state.slots).find((id) => id !== SLOT);
  // The Tab List chord is Cmd+Shift+L on macOS and Ctrl+Shift+L elsewhere.
  const isMac = await browser.execute(() => navigator.platform.startsWith("Mac"));
  const chord = isMac ? { meta: true, shift: true } : { ctrl: true, shift: true };

  const create = async () => {
    const previous = [...(await snapshot(observer)).tabPanelIds];
    observer.sendIntent("panel.tab.new", undefined, { slotId: SLOT, source: "clone" });
    await waitFor(async () => (await snapshot(observer)).tabPanelIds.length === previous.length + 1,
      { label: "new listing tab" });
    const panelId = observer.state.slots[SLOT].tabPanelIds.find((id) => !previous.includes(id));
    created.push(panelId);
    await browser.$(`[data-tab-panel-id="${panelId}"]`).waitForExist();
    return panelId;
  };

  const goTo = async (panelId, path) => {
    observer.sendIntent("NavigatePanelTo", undefined, { panelId, path });
    await waitFor(() => observer.state.panels[panelId]?.path === path, { label: `tab ${panelId} at ${path}` });
  };

  const openWithChord = async (panelId) => {
    await pressPanelKey(browser, panelId, "l", chord);
  };

  try {
    // A narrow strip plus enough tabs to overflow it: the only state in which
    // the count button exists.
    await browser.setWindowSize(800, 800);
    if (originalIds.length <= 3) {
      assert.equal(await exists(browser, countButton), false, "no count button while the strip fits");
    }
    for (let index = 0; index < 8; index++) await create();
    const tabs = [...(await snapshot(observer)).tabPanelIds];
    await browser.$(countButton).waitForExist();
    assert.equal(await (await browser.$(countButton)).getText(), String(tabs.length),
      "the count button shows the live tab count");

    // Distinct titles, so a filter query selects one tab rather than eight clones.
    await goTo(created[0], parentOf(leftPath));
    await goTo(created[1], parentOf(parentOf(leftPath)));

    // A slot with a single tab has nothing to list: the chord opens nothing.
    if (otherSlotId && observer.state.slots[otherSlotId].tabPanelIds.length === 1) {
      await openWithChord(observer.state.slots[otherSlotId].activePanelId);
      await sleep(400);
      assert.equal(await exists(browser, widget), false, "a one-tab slot opens nothing");
    }

    await markOpener(browser, observer);
    const activeNow = await activeTab(observer);

    // Open with the registry chord from the listing.
    await openWithChord(activeNow);
    await browser.$(input).waitForExist();
    await waitFor(() => browser.execute((selector) =>
      document.activeElement === document.querySelector(selector), input),
    { label: "the search field takes initial focus" });
    assert.deepEqual(await renderedRows(browser), tabs, "rows follow the tab-strip order");
    assert.equal(await browser.execute((selector) =>
      document.querySelector(selector).classList.contains("panel__tab-list-row--active"), rowOf(activeNow)),
    true, "the active tab's row is marked");

    // Panel and global shortcuts stay inert while the field owns the keys.
    await type(browser, "sjr");
    assert.equal(await (await browser.$(input)).getValue(), "sjr", "letters type, they do not run commands");
    assert.equal(await exists(browser, '[data-testid="panel-sort-selector"]'), false, "no sort selector over the list");
    assert.equal(await exists(browser, '[data-modal-id="jobs"]'), false, "no Jobs modal over the list");
    assert.equal(await exists(browser, ".context-menu"), false, "no context menu over the list");
    assert.equal(await exists(browser, widget), true, "the list is still open");

    // The query filters: fewer rows, all real tabs, matched letters marked.
    // It is the deeper tab's own folder name, so the shallower fixture tab
    // cannot match it by title or by path.
    const query = leafOf(parentOf(leftPath));
    assert.ok(query, `the fixture path ${leftPath} is too shallow to build a distinguishing filter`);
    await setTextInputValue(browser, input, query);
    const filtered = await renderedRows(browser);
    assert.ok(filtered.every((panelId) => tabs.includes(panelId)), "filtered rows are tabs of this slot");
    assert.ok(filtered.includes(created[0]), `the tab titled ${query} matches its title`);
    assert.ok(!filtered.includes(created[1]), `the folder above does not match ${query}`);
    assert.ok(filtered.length > 0 && filtered.length < tabs.length,
      `the filter narrows ${tabs.length} tabs to ${filtered.length}`);
    assert.equal(await browser.execute(() => !!document.querySelector(".panel__tab-list-match")), true,
      "matched characters are marked in the tab name");
    assert.equal(await exists(browser, ".panel__tab-list-empty"), false, "matches leave no empty message");

    // Home/End and the arrows move the highlight; Enter activates its tab.
    const first = await highlighted(browser);
    assert.ok(filtered.includes(first), "a row is highlighted after filtering");
    await browser.keys("End");
    assert.equal(await highlighted(browser), filtered[filtered.length - 1], "End highlights the last match");
    await browser.keys("ArrowUp");
    const target = await highlighted(browser);
    assert.notEqual(target, first, "ArrowUp moves the highlight");
    await browser.keys("Enter");
    await waitFor(async () => (await snapshot(observer)).activePanelId === target, { label: "Enter switches tabs" });
    await browser.$(widget).waitForExist({ reverse: true });
    // The newly active tab owns focus now; its predecessor closed with its tab.
    await waitFor(() => focusedIs(browser, target), { label: "the newly active tab takes focus" });

    // Escape closes the list and restores the exact opener.
    await markOpener(browser, observer);
    await openWithChord(await activeTab(observer));
    await browser.$(input).waitForExist();
    await browser.keys("Escape");
    await browser.$(widget).waitForExist({ reverse: true });
    await waitFor(() => focusIsOpener(browser), { label: "Escape restores the opener" });

    // Delete closes the highlighted row and keeps the list open.
    await openWithChord(await activeTab(observer));
    await browser.$(input).waitForExist();
    await highlightRow(browser, created[1]);
    await browser.keys("Delete");
    await waitFor(async () => !(await snapshot(observer)).tabPanelIds.includes(created[1]),
      { label: "Delete closes the highlighted tab" });
    assert.equal(await exists(browser, widget), true, "the list survives closing a row");
    assert.equal(await browser.execute((rows) => document.querySelectorAll(rows).length, rows),
      tabs.length - 1, "the closed row leaves the list");
    assert.equal(await (await browser.$(countButton)).getText(), String(tabs.length - 1),
      "the count updates after a close");
    await browser.keys("Escape");
    await browser.$(widget).waitForExist({ reverse: true });

    // A real click in the other panel closes the list and keeps that focus.
    await markOpener(browser, observer);
    const opener = await activeTab(observer);
    await openWithChord(opener);
    await browser.$(input).waitForExist();
    const otherPanel = observer.state.slots[otherSlotId]?.activePanelId ?? "right";
    await browser.$(panelBox(otherPanel)).click();
    await browser.$(widget).waitForExist({ reverse: true });
    await waitFor(() => focusedIs(browser, otherPanel), { label: "an outside click keeps the focus it took" });
    observer.sendIntent("SwitchFocus", undefined, { to: opener });
    await waitFor(async () => (await snapshot(observer)) && observer.state.focus === opener,
      { label: "the left panel is active again" });

    // The count button toggles the list and leaves the listing focused.
    await markOpener(browser, observer);
    await browser.$(countButton).click();
    await browser.$(input).waitForExist();
    await waitFor(() => browser.execute((selector) =>
      document.activeElement === document.querySelector(selector), input),
    { label: "the count button opens the list and focuses its field" });
    await browser.$(countButton).click();
    await browser.$(widget).waitForExist({ reverse: true });
    await waitFor(() => focusIsOpener(browser), { label: "the count button leaves the listing focused" });
  } finally {
    await browser.keys("Escape");
    for (const panelId of created) observer.sendIntent("panel.tab.close", undefined, { panelId });
    await waitFor(async () => JSON.stringify((await snapshot(observer)).tabPanelIds) === JSON.stringify(originalIds),
      { label: "tab fixtures closed" });
    observer.sendIntent("panel.tab.switch", undefined, { slotId: SLOT, panelId: originalActive });
    await snapshot(observer);
    await browser.setWindowSize(1280, 800);
    if (originalIds.length <= 3) {
      assert.equal(await exists(browser, countButton), false, "no count button once the strip fits again");
    }
    if (originalIds.length === 1) {
      await pressPanelKey(browser, originalActive, "l", chord);
      await sleep(400);
      assert.equal(await exists(browser, widget), false, "a single tab has nothing to list");
    }
    console.log("PASS tab list: count button, registry chord, inert panel keys, filter and marks, arrows, Enter switch, Delete close, Escape and outside-click focus return");
  }
}
