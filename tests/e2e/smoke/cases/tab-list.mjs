import assert from "node:assert/strict";
import { pressPanelKey, pressWindowKey, reloadBrowserApp, setTextInputValue } from "../browser.mjs";
import { sleep, waitFor } from "../wait.mjs";

const SLOT = "left";
/** Ceiling for the fixture tabs needed to overflow a very wide tab strip. */
const MAX_TAB_FIXTURES = 24;
const countButton = `[data-testid="panel-tabs-count-${SLOT}"]`;
const widget = '[data-testid="panel-tab-list"]';
const input = '[data-testid="panel-tab-list-input"]';
const rows = `${widget} [data-testid^="panel-tab-list-row-"]`;
const rowOf = (panelId) => `[data-testid="panel-tab-list-row-${panelId}"]`;
const panelBox = (panelId) => `[data-testid="panel-${panelId}"]`;
const tabsBar = `[data-testid="panel-tabs-${SLOT}"]`;

/** Whether the strip has room for every tab: with room, the count button is
 *  legitimately absent rather than missing. */
const stripFits = (browser) => browser.execute((bar) => {
  const el = document.querySelector(`${bar} .panel__tabs-list`);
  return !!el && el.scrollWidth <= el.clientWidth + 1;
}, tabsBar);

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
/** Whether the list's search field is the focused element. */
const fieldFocused = (browser) => browser.execute((selector) =>
  document.activeElement === document.querySelector(selector), input);

/** Wait for the backend to consider `panelId` the focused panel. A focus
 *  change the frontend asked for is answered on the frontend's own WebSocket
 *  connection, so no `focus` patch ever reaches this observer: the value has
 *  to be pulled back with a snapshot. */
async function waitFocused(observer, panelId) {
  let requestedAt = 0;
  await waitFor(async () => {
    if (Date.now() - requestedAt > 500) {
      requestedAt = Date.now();
      await snapshot(observer);
    }
    return observer.state.focus === panelId;
  }, { label: `panel ${panelId} active` });
}

/** Park focus on the slot's current listing and mark it as the expected opener. */
async function markOpener(browser, observer) {
  const panelId = await activeTab(observer);
  await browser.execute((selector) => {
    const panel = document.querySelector(selector);
    panel.focus();
    panel.dataset.tabListOpener = "true";
  }, panelBox(panelId));
  await waitFocused(observer, panelId);
  return panelId;
}

const highlighted = (browser) => browser.execute((rows) => {
  const selected = document.querySelector(`${rows}[data-selected="true"]`);
  return selected ? selected.dataset.testid.replace("panel-tab-list-row-", "") : null;
}, rows);

/** The row whose × button currently holds focus, or null. */
const armedOn = (browser) => browser.execute(() =>
  document.activeElement?.dataset.tabListClose ?? null);

const renderedRows = (browser) => browser.execute((rows) => Array.from(
  document.querySelectorAll(rows), (element) => element.dataset.testid.replace("panel-tab-list-row-", ""),
), rows);

/** What each rendered row actually shows, so a filter mismatch names its cause. */
const rowTexts = (browser) => browser.execute((rows) => Array.from(
  document.querySelectorAll(rows), (element) => ({
    id: element.dataset.testid.replace("panel-tab-list-row-", ""),
    title: element.querySelector(".panel__tab-list-title")?.textContent ?? "",
    detail: element.querySelector(".panel__tab-list-detail")?.textContent ?? "",
  }),
), rows);

/** The widget's own keep rule: the query survives as a substring or as a
 *  subsequence of the lowercased title or detail. */
const containsSubsequence = (text, needle) => {
  const hay = text.toLowerCase();
  let at = 0;
  for (const character of hay) {
    if (character === needle[at]) at++;
    if (at === needle.length) return true;
  }
  return false;
};

const expectedRowsFor = (rows, needle) => rows
  .filter(({ title, detail }) =>
    containsSubsequence(title, needle) || containsSubsequence(detail, needle))
  .map(({ id }) => id);

/** The filter state as the field and the list show it, for assertion messages. */
const filterState = async (browser) => `field ${JSON.stringify(await (await browser.$(input)).getValue())}, `
  + `rows ${JSON.stringify(await rowTexts(browser))}`;

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

  // The count button exists only while the strip overflows. The resize at the
  // start of the case may be ignored — a tiling compositor keeps the window as
  // wide as it likes — so the fixtures also grow until the strip runs out of
  // room at whatever width the window has.
  const overflowStrip = async () => {
    while (!(await exists(browser, countButton)) && created.length < MAX_TAB_FIXTURES) await create();
    await sleep(250);
    return exists(browser, countButton);
  };

  try {
    // A strip that overflows: the only state in which the count button exists.
    await browser.setWindowSize(800, 800);
    if (originalIds.length <= 3) {
      assert.equal(await exists(browser, countButton), false, "no count button while the strip fits");
    }
    for (let index = 0; index < 8; index++) await create();
    const width = await browser.execute(() => window.innerWidth);
    assert.ok(await overflowStrip(),
      `${created.length} tabs did not overflow a ${width}px-wide strip, so the window never got `
      + "narrow enough for this case to exercise an overflowing strip");
    const tabs = [...(await snapshot(observer)).tabPanelIds];
    assert.equal(await (await browser.$(countButton)).getText(), String(tabs.length),
      "the count button shows the live tab count");

    // Distinct titles, so a filter query selects one tab rather than eight clones.
    await goTo(created[0], parentOf(leftPath));
    await goTo(created[1], parentOf(parentOf(leftPath)));
    // Both navigations arrived over the observer's connection. The patch that
    // carries a panel's new path is answered to the initiating sender only
    // (navigate_to hands the scan that connection's sender, and scan.rs posts
    // the path-carrying start patch to it), so the app still believes both tabs
    // sit in the folder they were cloned from and names their rows by it. Pull
    // an authoritative snapshot before any row text is read or filtered.
    await reloadBrowserApp(browser, "fbro-a");

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
    const rowsBeforeLetters = await rowTexts(browser);
    await type(browser, "sjr");
    assert.equal(await (await browser.$(input)).getValue(), "sjr", "letters type, they do not run commands");
    // The letters have to reach the list's own state, not only the field, so
    // compare the rendered rows with what the same keep rule selects in Node.
    assert.deepEqual(await renderedRows(browser), expectedRowsFor(rowsBeforeLetters, "sjr"),
      `typed letters filter the list (${await filterState(browser)})`);
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
    assert.ok(filtered.includes(created[0]),
      `the tab titled ${query} matches its title (${await filterState(browser)})`);
    assert.ok(!filtered.includes(created[1]),
      `the folder above does not match ${query} (${await filterState(browser)})`);
    assert.ok(filtered.length > 0 && filtered.length < tabs.length,
      `the filter narrows ${tabs.length} tabs to ${filtered.length} (${await filterState(browser)})`);
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

    // A row's × is keyboard-only in the Tab sense and reached with Right:
    // Left hands focus back to the field, Right arms the × again, Enter
    // activates it, the list stays open, and focus goes back to the field.
    const liveNow = await activeTab(observer);
    const doomed = created.slice(2).find((panelId) => panelId !== liveNow);
    assert.ok(doomed, "a fixture tab that is not the active one can be closed from the list");
    await openWithChord(liveNow);
    await browser.$(input).waitForExist();
    await highlightRow(browser, created[1]);
    await browser.keys("ArrowRight");
    await waitFor(async () => (await armedOn(browser)) === created[1],
      { label: "Right moves focus onto that row's close button" });
    await browser.keys("ArrowLeft");
    await waitFor(() => fieldFocused(browser),
      { label: "Left hands focus back to the search field" });
    assert.equal(await highlighted(browser), created[1], "Left leaves the highlight alone");
    await browser.keys("ArrowRight");
    await waitFor(async () => (await armedOn(browser)) === created[1],
      { label: "Right arms the highlighted row again" });
    await browser.keys("Enter");
    await waitFor(async () => !(await snapshot(observer)).tabPanelIds.includes(created[1]),
      { label: "Enter on the close button closes that tab" });
    assert.equal(await exists(browser, widget), true, "the list survives closing a row");
    // created[1] is never the shown tab here: the filter dropped it, so the
    // Enter switch above landed on one of the other rows.
    assert.equal(await activeTab(observer), liveNow,
      "closing a tab that is not the shown one leaves the shown tab alone");
    // The armed × of the closed row is gone, and no other row's × replaces it:
    // the field takes focus back.
    await waitFor(() => fieldFocused(browser),
      { label: "closing a row hands focus back to the search field" });
    assert.equal(await armedOn(browser), null, "no close button keeps focus after its row closed");
    assert.equal(await browser.execute((rows) => document.querySelectorAll(rows).length, rows),
      tabs.length - 1, "the closed row leaves the list");

    // An × never follows the cursor: Right arms the highlighted row's ×, a
    // second Right leaves it alone, and the next list arrow moves the highlight
    // and hands focus back to the field. Delete then closes the highlighted row.
    await highlightRow(browser, doomed);
    await browser.keys("ArrowRight");
    await waitFor(async () => (await armedOn(browser)) === doomed,
      { label: "Right arms the highlighted row" });
    await browser.keys("ArrowRight");
    assert.equal(await armedOn(browser), doomed, "a second Right does not move the close button");
    assert.equal(await highlighted(browser), doomed, "a second Right leaves the highlight alone");
    await browser.keys("ArrowDown");
    await waitFor(() => fieldFocused(browser),
      { label: "ArrowDown hands focus back to the search field" });
    assert.equal(await armedOn(browser), null, "ArrowDown leaves the close button unfocused");
    const stepped = await highlighted(browser);
    assert.notEqual(stepped, doomed, "ArrowDown moves the highlight off the armed row");
    await browser.keys("ArrowRight");
    await waitFor(async () => (await armedOn(browser)) === stepped,
      { label: "Right arms the newly highlighted row" });
    // Home and the walk to a closable row release that × as well.
    await highlightRow(browser, doomed);
    assert.equal(await armedOn(browser), null, "the arrows give the close button back for good");
    assert.equal(await fieldFocused(browser), true, "and leave the search field focused");
    await browser.keys("Delete");
    await waitFor(async () => !(await snapshot(observer)).tabPanelIds.includes(doomed),
      { label: "Delete closes the highlighted tab" });
    assert.equal(await browser.execute((rows) => document.querySelectorAll(rows).length, rows),
      tabs.length - 2, "the second closed row leaves the list too");
    // Closing rows shrinks the strip, so the count either follows the tab count
    // or the button is gone because the surviving tabs fit again — never both
    // wrong: it may only disappear together with the overflow.
    const countLabel = await exists(browser, countButton)
      ? await (await browser.$(countButton)).getText()
      : null;
    if (countLabel === null) {
      assert.ok(await stripFits(browser), "the count button disappeared while the strip overflowed");
    } else {
      assert.equal(countLabel, String(tabs.length - 2), "the count updates after each close");
    }
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

    // The count button toggles the list and leaves the listing focused. The
    // closes above can leave the strip fitting again, so ask for the overflow
    // back rather than clicking a button the strip is entitled to hide — and do
    // it before marking the opener, because a new tab becomes the shown one.
    assert.ok(await overflowStrip(),
      "fixtures added after the closes still did not overflow the tab strip");
    await markOpener(browser, observer);
    await browser.$(countButton).click();
    await browser.$(input).waitForExist();
    await waitFor(() => browser.execute((selector) =>
      document.activeElement === document.querySelector(selector), input),
    { label: "the count button opens the list and focuses its field" });
    await browser.$(countButton).click();
    await browser.$(widget).waitForExist({ reverse: true });
    await waitFor(() => focusIsOpener(browser), { label: "the count button leaves the listing focused" });

    // The command palette opens the same list. It removes its own layer in the
    // commit that opens the list, which is exactly where an auto-close guard
    // reading a stale "modal open" value used to dismiss it again.
    await pressWindowKey(browser, "p", chord);
    await browser.$('[data-modal-id="command-palette"] input').waitForExist();
    await type(browser, "tab list");
    await browser.keys("Enter");
    await browser.$(input).waitForExist();
    // The list the palette opens holds the slot's tabs as they are now: the
    // fixture count has moved since the snapshot this case filtered against.
    const listedForPalette = (await snapshot(observer)).tabPanelIds;
    assert.equal(await browser.execute((rows) => document.querySelectorAll(rows).length, rows),
      listedForPalette.length, "the palette opens the focused slot's list");
    assert.deepEqual(await renderedRows(browser), listedForPalette,
      "the palette's list holds the live tabs of the focused slot");
    await waitFor(() => browser.execute((selector) =>
      document.activeElement === document.querySelector(selector), input),
    { label: "the palette-opened list focuses its own search field" });
    await browser.keys("Escape");
    await browser.$(widget).waitForExist({ reverse: true });
    await waitFor(() => focusIsOpener(browser), { label: "the palette-opened list restores the opener" });
    // Closing the tab the slot is showing is the one case that reseats it: the
    // slot takes the tab on its left, and the list goes with the panel it was
    // hosted in.
    const shown = await activeTab(observer);
    const shownIds = (await snapshot(observer)).tabPanelIds;
    const shownIndex = shownIds.indexOf(shown);
    const reseated = shownIndex > 0 ? shownIds[shownIndex - 1] : shownIds[shownIndex + 1];
    assert.ok(reseated, "the fixture slot has a tab to reseat onto");
    await openWithChord(shown);
    await browser.$(input).waitForExist();
    await highlightRow(browser, shown);
    await browser.keys("Delete");
    await waitFor(async () => (await snapshot(observer)).activePanelId === reseated,
      { label: "closing the shown tab moves the slot to its left neighbour" });
    await browser.$(widget).waitForExist({ reverse: true });
    assert.equal(await exists(browser, rowOf(shown)), false, "the closed tab's row goes with the list");
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
    console.log("PASS tab list: count button, registry chord, inert panel keys, filter and marks, arrows, Enter switch, Escape and outside-click focus return, palette open, Right arms a row's close button and Left hands it back, list arrows and a close return focus to the field, Enter and Delete close, closing a background row keeps the shown tab, closing the shown row reseats the slot left");
  }
}
