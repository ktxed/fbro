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

export async function clickCommand(browser, commandId) {
  const el = await browser.$(`[data-command-id="${cssString(commandId)}"]`);
  await el.waitForClickable({ timeout: 10_000 });
  await el.click();
}

export async function clickEntry(browser, panelId, name) {
  const el = await browser.$(`[data-testid="panel-${panelId}"] [data-testid="panel-entry"][data-entry-name="${cssString(name)}"]`);
  await el.waitForClickable({ timeout: 10_000 });
  await el.click();
  return el;
}

export async function doubleClickEntry(browser, panelId, name) {
  const el = await clickEntry(browser, panelId, name);
  await el.doubleClick();
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

export async function waitForNewWindow(browser, previousHandles, label = "new window") {
  const known = new Set(previousHandles);
  return waitFor(async () => {
    const handles = await browser.getWindowHandles();
    return handles.find((handle) => !known.has(handle)) ?? false;
  }, { timeout: 15_000, interval: 100, label });
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
