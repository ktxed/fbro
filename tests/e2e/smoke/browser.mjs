import { remote } from "webdriverio";
import { cssString, waitFor } from "./wait.mjs";

export async function waitForWebDriver(name, host, port) {
  await waitFor(async () => {
    try {
      const response = await fetch(`http://${host}:${port}/status`);
      return response.ok;
    } catch {
      return false;
    }
  }, {
    timeout: 60_000,
    interval: 500,
    label: `${name} WebDriver status`,
  });
}

export async function startBrowser(name, host, port, application) {
  await waitForWebDriver(name, host, port);
  const browser = await remote({
    protocol: "http",
    hostname: host,
    port,
    path: "/",
    logLevel: "warn",
    capabilities: {
      "tauri:options": { application },
    },
  });
  await browser.setWindowSize(1280, 800);
  await waitForMainWindow(browser, name);
  return browser;
}

async function waitForMainWindow(browser, name) {
  await waitFor(() => browser.$('[data-testid="panel-left"]').then((el) => el.isExisting()), {
    timeout: 30_000,
    label: `${name} main window`,
  });
}

export async function reloadBrowserApp(browser, name) {
  await browser.refresh();
  await waitForMainWindow(browser, name);
}

export async function clickSortHeader(browser, panelId, by) {
  const el = await browser.$(`[data-testid="panel-${panelId}"] .panel__sort-${by}`);
  await el.waitForClickable({ timeout: 10_000 });
  await el.click();
}

export async function openSortSelector(browser, panelId) {
  const panel = await browser.$(`[data-testid="panel-${panelId}"]`);
  await panel.waitForClickable({ timeout: 10_000 });
  await panel.click();
  await browser.performActions([{
    type: "key",
    id: "keyboard",
    actions: [
      { type: "keyDown", value: "\uE008" },
      { type: "keyDown", value: "s" },
      { type: "keyUp", value: "s" },
      { type: "keyUp", value: "\uE008" },
    ],
  }]);
  await browser.releaseActions();
  const selector = await browser.$(`[data-testid="panel-${panelId}"] [data-testid="panel-sort-selector"]`);
  await selector.waitForDisplayed({ timeout: 10_000 });
  return selector;
}

export async function clickCommand(browser, commandId) {
  const el = await browser.$(`[data-command-id="${cssString(commandId)}"]`);
  await el.waitForClickable({ timeout: 10_000 });
  await el.click();
}

export async function expectCheckboxSelected(browser, selector, expected, label) {
  const input = await browser.$(selector);
  await input.waitForDisplayed({ timeout: 10_000 });
  const selected = await input.isSelected();
  if (selected !== expected) {
    throw new Error(`${label} expected ${expected ? "checked" : "unchecked"}, got ${selected ? "checked" : "unchecked"}`);
  }
  return input;
}

export async function clickEntry(browser, panelId, name) {
  const el = await browser.$(`[data-testid="panel-${panelId}"] [data-testid="panel-entry"][data-entry-name="${cssString(name)}"]`);
  await el.waitForClickable({ timeout: 10_000 });
  await el.click();
  return el;
}

export async function clickContextMenuItem(browser, label) {
  const item = await waitFor(async () => {
    const items = await browser.$$(".context-menu [role='menuitem']");
    for (const candidate of items) {
      const text = (await candidate.getText()).trim();
      if (text.split("\n")[0] === label || text === label) return candidate;
    }
    return false;
  }, { timeout: 10_000, label: `context menu item ${label}` });
  await item.waitForClickable({ timeout: 10_000 });
  await item.click();
  return item;
}

export async function openPanelContextMenu(browser, panelId) {
  await browser.execute((panelId) => document.querySelector(`[data-testid="panel-${panelId}"]`)?.focus(), panelId);
  await browser.keys("c");
  const menu = await browser.$(".context-menu");
  await menu.waitForDisplayed({ timeout: 10_000 });
  return menu;
}

export async function longRightClickEntry(browser, panelId, name, holdMs = 500) {
  const el = await browser.$(`[data-testid="panel-${panelId}"] [data-testid="panel-entry"][data-entry-name="${cssString(name)}"]`);
  await el.waitForDisplayed({ timeout: 10_000 });
  await browser.executeAsync(({ panelId, name, holdMs }, done) => {
    const escaped = globalThis.CSS?.escape ? CSS.escape(name) : String(name).replace(/\\/g, "\\\\").replace(/"/g, '\\"');
    const el = document.querySelector(`[data-testid="panel-${panelId}"] [data-testid="panel-entry"][data-entry-name="${escaped}"]`);
    if (!el) {
      done(`missing entry ${name}`);
      return;
    }
    const rect = el.getBoundingClientRect();
    const eventInit = {
      bubbles: true,
      cancelable: true,
      button: 2,
      buttons: 2,
      clientX: rect.left + 4,
      clientY: rect.top + 4,
      view: window,
    };
    el.dispatchEvent(new MouseEvent("mousedown", eventInit));
    window.setTimeout(() => {
      el.dispatchEvent(new MouseEvent("mouseup", { ...eventInit, buttons: 0 }));
      el.dispatchEvent(new MouseEvent("contextmenu", { ...eventInit, buttons: 0 }));
      done(null);
    }, holdMs);
  }, { panelId, name, holdMs });
  const menu = await browser.$(".context-menu");
  await menu.waitForDisplayed({ timeout: 10_000 });
  return menu;
}

export async function clickSource(browser, panelId, sourceId) {
  const el = await browser.$(`[data-testid="panel-${panelId}"] [data-source-id="${cssString(sourceId)}"] .panel__volume-btn`);
  await el.waitForClickable({ timeout: 10_000 });
  await el.click();
  return el;
}

export async function doubleClickEntry(browser, panelId, name) {
  const el = await clickEntry(browser, panelId, name);
  await el.doubleClick();
}

export async function setTextInputValue(browser, selector, value) {
  const input = await browser.$(selector);
  await input.waitForDisplayed({ timeout: 10_000 });
  await browser.execute(({ selector, value }) => {
    const el = document.querySelector(selector);
    if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) {
      throw new Error(`Expected text input for selector: ${selector}`);
    }
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const valueSetter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
    valueSetter?.call(el, value);
    el.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      cancelable: true,
      data: value,
      inputType: "insertReplacementText",
    }));
  }, { selector, value });
  await waitFor(async () => await input.getValue() === value, {
    timeout: 5_000,
    label: `${selector} value ${value}`,
  });
  return input;
}

export async function pressPanelKey(browser, panelId, key, modifiers = {}) {
  await browser.execute(({ panelId, key, modifiers }) => {
    const el = document.querySelector(`[data-testid="panel-${panelId}"]`);
    el?.focus();
    el?.dispatchEvent(new KeyboardEvent("keydown", {
      key,
      code: key.startsWith("F") ? key : undefined,
      shiftKey: !!modifiers.shift,
      ctrlKey: !!modifiers.ctrl,
      altKey: !!modifiers.alt,
      metaKey: !!modifiers.meta,
      bubbles: true,
      cancelable: true,
    }));
  }, { panelId, key, modifiers });
}

export async function pressWindowKey(browser, key, modifiers = {}) {
  await browser.execute(({ key, modifiers }) => {
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key,
      code: key.startsWith("F") ? key : undefined,
      shiftKey: !!modifiers.shift,
      ctrlKey: !!modifiers.ctrl,
      altKey: !!modifiers.alt,
      metaKey: !!modifiers.meta,
      bubbles: true,
      cancelable: true,
    }));
  }, { key, modifiers });
}

export async function createDirectory(browser, observer, panelId, name) {
  await browser.execute((panelId) => document.querySelector(`[data-testid="panel-${panelId}"]`)?.focus(), panelId);
  await clickCommand(browser, "file.mkdir");
  const input = await browser.$('[data-testid="mkdir-name-input"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await input.setValue(name);
  await clickCommand(browser, "file.mkdir.confirm");
  await observer.waitForEntry(panelId, name, "directory");
}

export async function createFile(browser, observer, panelId, name) {
  await pressPanelKey(browser, panelId, "F4", { shift: true });
  const input = await browser.$('[data-testid="touch-name-input"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await input.setValue(name);
  await clickCommand(browser, "file.touch.confirm");
  await observer.waitForEntry(panelId, name, "file");
}

export async function confirmOperation(browser, commandId) {
  const dialog = await browser.$('[data-testid="operation-dialog"]');
  await dialog.waitForDisplayed({ timeout: 10_000 });
  await clickCommand(browser, commandId);
}

export async function closeTerminalJobDialog(browser) {
  const close = await waitFor(async () => {
    const el = await browser.$('[data-command-id="job.close"]');
    return await el.isExisting() ? el : false;
  }, { timeout: 5_000, interval: 100, label: "terminal job close button" }).catch(() => null);
  if (close) {
    await close.click();
  }
}
