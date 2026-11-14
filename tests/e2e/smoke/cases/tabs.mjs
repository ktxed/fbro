import assert from "node:assert/strict";
import { pressWindowKey } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

const bar = '[data-testid="panel-tabs-left"]';
const chip = (id) => `${bar} [data-tab-panel-id="${id}"]`;

async function snapshot(observer) {
  const start = observer.messages.length;
  observer.sendIntent("RequestStateSnapshot");
  await waitFor(() => observer.messages.slice(start).some((m) => m.type === "StateSnapshot"),
    { label: "tabs authoritative snapshot" });
  return observer.state.slots.left;
}

async function pointer(browser, actions) {
  await browser.performActions([{ type: "pointer", id: "tab-mouse", parameters: { pointerType: "mouse" }, actions }]);
}

async function point(browser, selector, fraction = 0.5) {
  return browser.execute((selector, fraction) => {
    const r = document.querySelector(selector).getBoundingClientRect();
    return { x: Math.round(r.left + r.width * fraction), y: Math.round(r.top + r.height / 2) };
  }, selector, fraction);
}

async function startDrag(browser, source, target) {
  await pointer(browser, [
    { type: "pointerMove", duration: 0, origin: "viewport", ...source },
    { type: "pointerDown", button: 0 },
    { type: "pointerMove", duration: 250, origin: "viewport", ...target },
  ]);
}

async function release(browser) {
  await pointer(browser, [{ type: "pointerUp", button: 0 }]);
  await browser.releaseActions();
}

export async function runTabsFlow(browser, observer) {
  const original = await snapshot(observer);
  const originalIds = [...original.tabPanelIds];
  const originalActive = original.activePanelId;
  const scheme = observer.state.settings.general.colorScheme;
  const created = [];
  const expectOrder = async (expected) => {
    await waitFor(async () => JSON.stringify((await snapshot(observer)).tabPanelIds) === JSON.stringify(expected),
      { label: `backend tab order ${expected}` });
    await waitFor(() => browser.execute((selector, ids) => JSON.stringify(Array.from(
      document.querySelectorAll(`${selector} [data-tab-panel-id]`), (el) => el.dataset.tabPanelId,
    )) === JSON.stringify(ids), bar, expected), { label: "rendered backend tab order" });
  };
  const create = async () => {
    const previous = [...(await snapshot(observer)).tabPanelIds];
    observer.sendIntent("panel.tab.new", undefined, { slotId: "left", source: "clone" });
    await waitFor(async () => (await snapshot(observer)).tabPanelIds.length === previous.length + 1,
      { label: "new listing tab" });
    const id = observer.state.slots.left.tabPanelIds.find((id) => !previous.includes(id));
    created.push(id);
    await browser.$(chip(id)).waitForExist();
    return id;
  };
  const reorder = async (source, target, fraction = 0.1) => {
    const previous = [...(await snapshot(observer)).tabPanelIds];
    await startDrag(browser, await point(browser, `${chip(source)} .panel__tab-label`), await point(browser, chip(target), fraction));
    await browser.$(`${bar} [data-testid="tab-drop-marker"]`).waitForExist();
    await expectOrder(previous);
    await release(browser);
  };

  try {
    const a = await create();
    const b = await create();
    const c = await create();
    const initial = [...originalIds, a, b, c];
    await expectOrder(initial);
    // Leave focus in the OTHER panel: reordering must preserve its exact owner.
    await browser.execute(() => document.querySelector('[data-panel-id="right"]')?.focus());
    await waitFor(async () => (await snapshot(observer)) && observer.state.focus === "right", { label: "right panel activation" });
    await waitFor(() => browser.execute(() => !!document.activeElement?.closest('[data-panel-id="right"]')), { label: "right panel focus" });
    const focus = await browser.execute(() => {
      document.activeElement.dataset.tabReorderOpener = "true";
      return document.activeElement.outerHTML;
    });
    const cursor = observer.state.panels.right.cursor;
    for (const colorScheme of ["fbro-dark", "daylight"]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: colorScheme });
      await waitFor(() => browser.execute((s) => document.querySelector(".app")?.dataset.colorScheme === s, colorScheme),
        { label: `tab reorder ${colorScheme}` });
      await reorder(c, a);
      await expectOrder([...originalIds, c, a, b]);
      assert.equal((await snapshot(observer)).activePanelId, c);
      await reorder(a, b, 0.9);
      await expectOrder([...originalIds, c, b, a]);
      assert.equal((await snapshot(observer)).activePanelId, c, "inactive drag must not activate tab");
      assert.equal(await browser.execute(() => document.activeElement?.dataset.tabReorderOpener), "true", focus);
      assert.equal(observer.state.panels.right.cursor, cursor);
      await reorder(b, c);
      await reorder(a, b);
      await expectOrder(initial);
    }

    // An ordinary click still switches, and a tiny move stays an ordinary click.
    const source = await point(browser, chip(a));
    await startDrag(browser, source, { x: source.x + 2, y: source.y });
    await release(browser);
    await waitFor(async () => (await snapshot(observer)).activePanelId === a, { label: "tab click switches" });
    await expectOrder(initial);

    for (const cancel of ["escape", "outside", "pointercancel", "blur"]) {
      await startDrag(browser, await point(browser, chip(c)), await point(browser, chip(a), 0.1));
      await browser.$('[data-testid="tab-drop-marker"]').waitForExist();
      if (cancel === "escape") await browser.keys("Escape");
      if (cancel === "outside") await pointer(browser, [{ type: "pointerMove", duration: 100, origin: "viewport", x: 500, y: 300 }]);
      if (cancel === "pointercancel") await browser.execute((selector) => document.querySelector(selector)
        .dispatchEvent(new PointerEvent("pointercancel", { bubbles: true })), chip(c));
      if (cancel === "blur") await browser.execute(() => window.dispatchEvent(new Event("blur")));
      await release(browser);
      await browser.$('[data-testid="tab-drop-marker"]').waitForExist({ reverse: true });
      await expectOrder(initial);
      assert.equal((await snapshot(observer)).activePanelId, a, `${cancel} must not switch tabs`);
    }

    // A modal takes input ownership from an in-progress drag.
    await startDrag(browser, await point(browser, chip(c)), await point(browser, chip(a), 0.1));
    const isMac = await browser.execute(() => navigator.platform.startsWith("Mac"));
    await pressWindowKey(browser, ",", isMac ? { meta: true } : { ctrl: true });
    await browser.$('[data-modal-id="settings"]').waitForExist();
    await browser.$('[data-testid="tab-drop-marker"]').waitForExist({ reverse: true });
    await release(browser);
    await browser.keys("Escape");
    await browser.$('[data-modal-id="settings"]').waitForExist({ reverse: true });
    await expectOrder(initial);

    // Closing a sibling concurrently cancels the gesture; it cannot resurrect it.
    const transient = await create();
    const beforeClose = [...(await snapshot(observer)).tabPanelIds];
    await startDrag(browser, await point(browser, chip(c)), await point(browser, chip(a), 0.1));
    observer.sendIntent("panel.tab.close", undefined, { panelId: transient });
    await expectOrder(beforeClose.filter((id) => id !== transient));
    await browser.$('[data-testid="tab-drop-marker"]').waitForExist({ reverse: true });
    await release(browser);

    // Keep a promoted terminal running and focused while moving its chip.
    observer.sendIntent("OpenPanelTerminal", a);
    await waitFor(async () => { await snapshot(observer); return observer.state.panels[a].terminalSessionId != null; },
      { label: "embedded terminal opened" });
    const sessionId = observer.state.panels[a].terminalSessionId;
    observer.sendIntent("panel.terminal.promote", undefined, { panelId: a });
    let terminal;
    await waitFor(async () => {
      terminal = (await snapshot(observer)).tabPanelIds.find((id) => observer.state.panels[id]?.tabKind === "terminal");
      return terminal;
    }, { label: "promoted terminal tab" });
    created.push(terminal);
    await browser.$(`${chip(terminal)} .panel__tab-label`).waitForExist();
    await waitFor(() => browser.execute(() => !!document.activeElement?.closest(".panel-terminal")),
      { label: "terminal owns keyboard focus" });
    await browser.execute(() => { document.activeElement.dataset.tabReorderOpener = "terminal"; });
    const terminalStart = [...(await snapshot(observer)).tabPanelIds];
    await reorder(terminal, originalIds[0]);
    await expectOrder([terminal, ...terminalStart.filter((id) => id !== terminal)]);
    assert.equal((await snapshot(observer)).activePanelId, terminal);
    assert.equal(observer.state.panels[terminal].terminalSessionId, sessionId);
    assert.equal(await browser.execute(() => document.activeElement?.dataset.tabReorderOpener), "terminal");
    await browser.keys("exit");
    await browser.keys("Enter");
    await expectOrder(initial);

    // A narrow window and many tabs exercise edge scrolling with a stationary pointer.
    await browser.setWindowSize(800, 800);
    const extra = [];
    for (let i = 0; i < 8; i++) extra.push(await create());
    const overflowOrder = [...(await snapshot(observer)).tabPanelIds];
    await browser.execute((selector) => { document.querySelector(`${selector} .panel__tabs-list`).scrollLeft = 0; }, bar);
    await startDrag(browser, await point(browser, `${chip(originalIds[0])} .panel__tab-label`), { x: 500, y: 300 });
    await release(browser);
    await browser.$(`${bar} [aria-label="Scroll tabs right"]`).click();
    await waitFor(() => browser.execute((selector) => document.querySelector(`${selector} .panel__tabs-list`).scrollLeft > 0, bar),
      { label: "fresh arrow click after cancelled tab drag" });
    await browser.execute((selector) => { document.querySelector(`${selector} .panel__tabs-list`).scrollLeft = 0; }, bar);
    const edge = await browser.execute((selector) => {
      const list = document.querySelector(`${selector} .panel__tabs-list`);
      const r = list.getBoundingClientRect();
      if (list.scrollWidth <= list.clientWidth) throw new Error("fixture tabs must overflow");
      return { x: Math.floor(r.right - 3), y: Math.round(r.top + r.height / 2) };
    }, bar);
    await startDrag(browser, await point(browser, `${chip(originalIds[0])} .panel__tab-label`), edge);
    await waitFor(() => browser.execute((selector) => {
      const list = document.querySelector(`${selector} .panel__tabs-list`);
      return list.scrollLeft >= list.scrollWidth - list.clientWidth - 1;
    }, bar), { label: "tab drag edge scroll reaches last tab" });
    await release(browser);
    await expectOrder([...overflowOrder.filter((id) => id !== originalIds[0]), originalIds[0]]);
    // Restore original position before removing the overflow fixtures.
    observer.sendIntent("ReorderPanelTab", undefined, { slotId: "left", panelId: originalIds[0], beforePanelId: overflowOrder[1] });
    await expectOrder(overflowOrder);
    for (const panelId of extra) observer.sendIntent("panel.tab.close", undefined, { panelId });
    await expectOrder(initial);
    await browser.setWindowSize(1280, 800);
    await browser.execute((selector) => { document.querySelector(`${selector} .panel__tabs-list`).scrollLeft = 0; }, bar);

    await browser.refresh();
    await waitFor(() => browser.execute(() => !!document.querySelector('.app[data-ready="true"]')),
      { label: "tabbed application reload" });
    await expectOrder(initial);
    // Closing via the button and middle click retains its original meaning.
    await browser.$(`${chip(c)} [aria-label="Close tab"]`).click();
    await expectOrder([...originalIds, a, b]);
    // WebKitWebDriver on Wayland reports native middle-up as button 0 and
    // leaves button 1 held. Exercise auxclick directly; drags use real actions.
    await browser.execute((selector) => document.querySelector(selector).dispatchEvent(
      new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }),
    ), chip(b));
    await expectOrder([...originalIds, a]);
    // Double-click on empty strip space creates exactly one new tab.
    const prior = [...(await snapshot(observer)).tabPanelIds];
    const empty = await point(browser, `${bar} .panel__tabs-list`, 0.95);
    await pointer(browser, [
      { type: "pointerMove", duration: 0, origin: "viewport", ...empty },
      { type: "pointerDown", button: 0 }, { type: "pointerUp", button: 0 },
      { type: "pointerDown", button: 0 }, { type: "pointerUp", button: 0 },
    ]);
    await browser.releaseActions();
    await waitFor(async () => (await snapshot(observer)).tabPanelIds.length === prior.length + 1,
      { label: "empty strip double click creates tab" });
    created.push(observer.state.slots.left.tabPanelIds.find((id) => !prior.includes(id)));

    // Tab context menu (mouse only): Duplicate copies the clicked tab right
    // after it, Close closes it, Escape dismisses and keeps the listing focus.
    observer.sendIntent("panel.tab.switch", undefined, { slotId: "left", panelId: a });
    await waitFor(async () => (await snapshot(observer)).activePanelId === a, { label: "menu fixture tab active" });
    const listingFocused = (panelId) => browser.execute((id) =>
      !!document.activeElement?.closest(`[data-panel-id="${id}"]`), panelId);
    const tabMenu = '[data-testid="tab-context-menu"]';
    // Like auxclick above: a WebDriver right click on WebKitWebDriver/Wayland
    // leaves input state behind that drops Shift from later typed keys ("!"
    // arrives as "1" in the editor flow), so dispatch the event directly.
    const contextClick = (selector) => browser.execute((selector) => {
      const el = document.querySelector(selector);
      const r = el.getBoundingClientRect();
      el.dispatchEvent(new MouseEvent("contextmenu", {
        bubbles: true, cancelable: true, button: 2,
        clientX: r.left + r.width / 2, clientY: r.top + r.height / 2,
      }));
    }, selector);
    const beforeDuplicate = [...(await snapshot(observer)).tabPanelIds];
    await contextClick(chip(a));
    await browser.$(tabMenu).waitForExist();
    const menuLabels = await browser.execute((selector) => Array.from(
      document.querySelectorAll(`${selector} [role="menuitem"]`), (el) => el.textContent), tabMenu);
    assert.deepEqual(menuLabels, ["Duplicate", "Close"]);
    // The menu owns input: a panel key underneath stays inert.
    const cursorBeforeMenuKey = observer.state.panels[a].cursor;
    await browser.keys("End");
    assert.equal((await snapshot(observer)) && observer.state.panels[a].cursor, cursorBeforeMenuKey);
    await browser.$('[data-testid="tab-menu-duplicate"]').click();
    await browser.$(tabMenu).waitForExist({ reverse: true });
    let duplicate;
    await waitFor(async () => {
      duplicate = (await snapshot(observer)).tabPanelIds.find((id) => !beforeDuplicate.includes(id));
      return duplicate;
    }, { label: "duplicated tab" });
    created.push(duplicate);
    const afterDuplicate = observer.state.slots.left.tabPanelIds;
    assert.equal(afterDuplicate.indexOf(duplicate), afterDuplicate.indexOf(a) + 1, "duplicate sits right after its origin");
    assert.equal(observer.state.panels[duplicate].path, observer.state.panels[a].path);
    await contextClick(chip(duplicate));
    await browser.$('[data-testid="tab-menu-close"]').click();
    await expectOrder(beforeDuplicate);
    await waitFor(() => listingFocused(a), { label: "tab menu close returns focus to the listing" });
    await contextClick(chip(a));
    await browser.$(tabMenu).waitForExist();
    await browser.keys("Escape");
    await browser.$(tabMenu).waitForExist({ reverse: true });
    await waitFor(() => listingFocused(a), { label: "tab menu Escape returns focus to the listing" });
    await expectOrder(beforeDuplicate);

    // Breadcrumb context menu: Copy path everywhere, Navigate to except on the leaf.
    const crumbs = `[data-panel-id="${a}"] [data-testid^="breadcrumb-"]`;
    const crumbMenu = '[data-testid="breadcrumb-context-menu"]';
    const startPath = observer.state.panels[a].path;
    const crumbCount = await browser.execute((selector) => document.querySelectorAll(selector).length, crumbs);
    assert.ok(crumbCount >= 2, `fixture path ${startPath} needs a parent breadcrumb`);
    await contextClick(`[data-panel-id="${a}"] [data-testid="breadcrumb-${crumbCount - 1}"]`);
    await browser.$(crumbMenu).waitForExist();
    assert.equal(await browser.$('[data-testid="breadcrumb-menu-navigate"]').isExisting(), false, "leaf has no Navigate to");
    await browser.$('[data-testid="breadcrumb-menu-copy-path"]').click();
    await waitFor(() => observer.state.notifications.entries.some((entry) => /^(Copied|Couldn't copy) path/.test(entry.message)),
      { label: "breadcrumb copy feedback" });
    await waitFor(() => listingFocused(a), { label: "breadcrumb menu returns focus to the listing" });
    const parentCrumb = `[data-panel-id="${a}"] [data-testid="breadcrumb-${crumbCount - 2}"]`;
    const parentPath = await browser.$(parentCrumb).getAttribute("title");
    await contextClick(parentCrumb);
    await browser.$('[data-testid="breadcrumb-menu-navigate"]').click();
    await waitFor(async () => (await snapshot(observer)) && observer.state.panels[a].path === parentPath,
      { label: "breadcrumb Navigate to" });
    observer.sendIntent("NavigatePanelTo", a, { panelId: a, path: startPath });
    await waitFor(async () => (await snapshot(observer)) && observer.state.panels[a].path === startPath,
      { label: "breadcrumb fixture restored" });
  } finally {
    await browser.releaseActions();
    await browser.setWindowSize(1280, 800);
    await browser.keys("Escape");
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
    for (const panelId of created) observer.sendIntent("panel.tab.close", undefined, { panelId });
    observer.sendIntent("panel.tab.switch", undefined, { slotId: "left", panelId: originalActive });
    await snapshot(observer);
  }
}
