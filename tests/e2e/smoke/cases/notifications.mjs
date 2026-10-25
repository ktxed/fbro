import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { backendPath, hostPath } from "../config.mjs";
import { clickEntry, goToPanelPath, pressWindowKey } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

async function request(observer, name, args = {}) {
  const requestId = randomUUID();
  observer.sendIntent(name, undefined, { ...args, requestId });
  return waitFor(() => observer.messages.find((message) => message.requestId === requestId && message.type === `${name}Ok`),
    { timeout: 10_000, label: `${name} acknowledgement` });
}

export async function waitForToast(browser, text) {
  await waitFor(() => browser.execute((text) => document.querySelector('[data-testid="transient-hint"]')?.textContent.includes(text), text),
    { timeout: 20_000, label: `queued toast ${text}` });
}

async function expectFocused(browser, selector, label) {
  try {
    await waitFor(() => browser.execute((selector) => document.querySelector(selector) === document.activeElement, selector),
      { timeout: 5_000, label });
  } catch (error) {
    const focused = await browser.execute(() => document.activeElement?.outerHTML);
    throw new Error(`${error.message}; focused: ${focused}`);
  }
}

async function openHistory(browser) {
  await browser.keys("j");
  await (await browser.$('[data-modal-id="jobs"]')).waitForDisplayed({ timeout: 5_000 });
  // Initial focus is Close. Tab wraps to the tablist, and End opens the last tab.
  await browser.keys("Tab");
  await browser.keys("End");
  await (await browser.$('[data-testid="notification-history"]')).waitForDisplayed({ timeout: 5_000 });
}

async function verifyOsNotificationSettings(browser, observer) {
  const previous = observer.state.settings.general.osNotificationsEnabled;
  await pressWindowKey(browser, ",", { ctrl: true });
  await pressWindowKey(browser, ",", { meta: true });
  await browser.$('[data-modal-id="settings"]').waitForDisplayed({ timeout: 10_000 });
  const selector = '[data-testid="os-notifications-enabled"]';
  try {
    const supported = observer.state.settings.general.supportsOsNotifications;
    const settingsAvailable = observer.state.settings.general.supportsOsNotificationSettings;
    const platform = observer.state.settings.general.osNotificationPlatform;
    const noticeSelector = platform === "windows"
      ? '[data-testid="windows-notifications-notice"]'
      : '[data-testid="macos-notifications-notice"]';
    if (await browser.$(selector).isExisting() !== (supported || settingsAvailable)) {
      throw new Error("OS notification setting visibility does not match backend capability");
    }
    if (await browser.$('[data-testid="open-os-notification-settings"]').isExisting() !== settingsAvailable) {
      throw new Error("OS notification guidance/action visibility does not match backend capability");
    }
    if (supported || settingsAvailable) {
      const help = '[data-testid="notification-help"]';
      const tooltip = '#notification-help-tooltip';
      if (await browser.$(tooltip).isDisplayed()) throw new Error("Notification help should not occupy space initially");
      await browser.execute((selector) => document.querySelector(selector)?.focus(), help);
      await browser.$(tooltip).waitForDisplayed();
      if (await browser.$(noticeSelector).isExisting() !== settingsAvailable) {
        throw new Error("Notification help does not match the backend platform capability");
      }
      if (settingsAvailable && !supported) {
        await browser.$('[data-testid="os-notifications-unavailable"]').waitForDisplayed();
      }
      await browser.keys("Escape");
      await browser.$(tooltip).waitForDisplayed({ reverse: true });
      await browser.$('[data-modal-id="settings"]').waitForDisplayed();
      await expectFocused(browser, help, "Escape dismisses help without closing Settings or moving focus");
    }
    if (settingsAvailable && !supported) {
      if (await browser.$(selector).isEnabled()) {
        throw new Error("Unavailable native delivery must disable the preference control");
      }
    }
    if (supported) {
      // Native Space activation owns the checkbox, and only backend patches update it.
      await browser.execute((selector) => document.querySelector(selector)?.focus(), selector);
      await browser.keys(" ");
      await waitFor(() => observer.state.settings.general.osNotificationsEnabled === !previous,
        { label: "keyboard toggles backend OS notification setting" });
      await waitFor(async () => await browser.$(selector).isSelected() === !previous,
        { label: "OS notification checkbox reflects backend patch" });
      await expectFocused(browser, selector, "OS notification setting keeps focus after patch");
      await browser.keys(" ");
      await waitFor(() => observer.state.settings.general.osNotificationsEnabled === previous,
        { label: "keyboard restores OS notification preference" });
      if (settingsAvailable) {
        await browser.keys("Tab");
        await expectFocused(browser, '[data-testid="open-os-notification-settings"]',
          "Tab reaches macOS notification settings after the delivery preference");
        await browser.keys(["Shift", "Tab"]);
        await expectFocused(browser, selector, "Shift+Tab returns to the delivery preference");
      }
    }
    const start = observer.messages.length;
    observer.sendIntent("SetGeneralOption", undefined, { key: "osNotificationsEnabled", value: "false" });
    await waitFor(() => observer.messages.slice(start).some((message) => message.type === "Error" && message.code === "BAD_SETTING"),
      { label: "OS notification setting rejects a non-boolean value" });
    if (observer.state.settings.general.osNotificationsEnabled !== previous) {
      throw new Error("invalid OS notification preference changed backend state");
    }
    const requestId = randomUUID();
    observer.sendIntent("PublishNotification", undefined, { requestId, message: "disallowed OS publication", channels: ["os"] });
    await waitFor(() => observer.messages.some((message) => message.type === "PublishNotificationFailed"
      && message.requestId === requestId && message.code === "INVALID_NOTIFICATION"),
    { label: "OS notification delivery remains backend-selected" });
  } finally {
    observer.sendIntent("SetGeneralOption", undefined, { key: "osNotificationsEnabled", value: previous });
    await browser.keys("Escape");
    await browser.$('[data-modal-id="settings"]').waitForDisplayed({ reverse: true });
  }
  await expectFocused(browser, '[data-testid="panel-left"]', "OS notification settings returns to its panel opener");
}

export async function runNotificationsFlow(browser, observer) {
  const previousPath = observer.state.panels.left.path;
  const previousScheme = observer.state.settings.general.colorScheme;
  const fixture = fs.mkdtempSync(hostPath(backendPath("a", "src", "notifications-")));
  const directory = backendPath("a", "src", path.basename(fixture));
  fs.writeFileSync(path.join(fixture, "note.txt"), "notification fixture\n");
  try {
    await goToPanelPath(browser, observer, "left", directory);
    await clickEntry(browser, "left", "note.txt");
    await verifyOsNotificationSettings(browser, observer);
    await waitFor(() => browser.execute(() => !document.querySelector('[data-testid="transient-hint"]')),
      { timeout: 20_000, label: "initial toast queue drained" });
    await request(observer, "ClearNotifications");

    // Actual frontend clipboard callsite: one publication under React StrictMode.
    await browser.keys("i");
    await (await browser.$('[data-modal-id="info"]')).waitForDisplayed({ timeout: 5_000 });
    await (await browser.$('[data-modal-id="info"] [aria-label="Copy full path"]')).click();
    await waitForToast(browser, "Copied full path");
    await expectFocused(browser, '[data-modal-id="info"] [aria-label="Copy full path"]', "clipboard toast preserves focus");
    await waitFor(() => observer.state.notifications.entries.some((entry) => entry.message === "Copied full path"),
      { label: "frontend copy recorded in backend history" });
    if (observer.state.notifications.entries.filter((entry) => entry.message === "Copied full path").length !== 1) throw new Error("frontend publication duplicated");
    await browser.keys("Escape");
    await expectFocused(browser, '[data-testid="panel-left"]', "copy modal returns to opener");
    await waitFor(() => browser.execute(() => !document.querySelector('[data-testid="transient-hint"]')),
      { timeout: 10_000, label: "clipboard toast expired" });

    await openHistory(browser);
    const copyRow = await browser.$('[data-testid="notification-entry"]');
    if (!(await copyRow.getText()).includes("Copied full path")) throw new Error("expired clipboard feedback missing from history");
    const timestamp = await browser.execute(() => document.querySelector('[data-testid="notification-entry"] time')?.dateTime);
    if (!Number.isFinite(Date.parse(timestamp))) throw new Error("notification lacks a valid timestamp");
    // Tab skips the tablist's other tabs and lands on Clear history.
    await browser.keys("Tab");
    await expectFocused(browser, '[data-testid="notifications-clear"]', "history clear is keyboard accessible");
    await browser.keys("Enter");
    await waitFor(() => observer.state.notifications.entries.length === 0, { label: "keyboard clear acknowledged" });
    await expectFocused(browser, '[data-testid="notifications-list"]', "clear hands focus to the persistent history list");

    await request(observer, "PublishNotification", { message: "history-only marker", channels: [] });
    await waitFor(() => browser.execute(() => document.querySelector('[data-testid="notifications-list"]')?.textContent.includes("history-only marker")),
      { label: "history-only entry displayed" });
    await expectFocused(browser, '[data-testid="notifications-list"]', "history list is one Tab stop");
    await browser.keys("Tab");
    await expectFocused(browser, '[data-modal-id="jobs"] .modal__actions button', "history rows add no extra Tab stops");
    await browser.performActions([{ type: "key", id: "keyboard", actions: [
      { type: "keyDown", value: "\uE008" }, { type: "keyDown", value: "\uE004" },
      { type: "keyUp", value: "\uE004" }, { type: "keyUp", value: "\uE008" },
    ] }]);
    await browser.releaseActions();
    await expectFocused(browser, '[data-testid="notifications-list"]', "Shift Tab returns to history list");
    const selectedId = await browser.execute(() => document.querySelector('[data-testid="notifications-list"]')?.getAttribute("aria-activedescendant"));
    const beforeCursor = observer.state.panels.left.cursor;

    // Arrival order differs from history display order; every toast must deliver.
    await request(observer, "PublishNotification", { message: "burst first" });
    await waitForToast(browser, "burst first");
    await request(observer, "PublishNotification", { message: "burst second", level: "warning" });
    await request(observer, "PublishNotification", { message: "burst third", level: "error" });
    await waitFor(() => browser.execute(() => document.querySelector('[data-testid="transient-hint"]')?.getAttribute("data-pending-count") === "3"),
      { label: "all burst toasts queued" });
    await expectFocused(browser, '[data-testid="notifications-list"]', "live notifications preserve history focus");
    const afterId = await browser.execute(() => document.querySelector('[data-testid="notifications-list"]')?.getAttribute("aria-activedescendant"));
    if (afterId !== selectedId) throw new Error("arrival changed the browsed notification");
    await browser.keys("Home");
    await browser.keys("ArrowDown");
    await browser.keys("End");
    const snapshotStart = observer.messages.length;
    observer.sendIntent("RequestStateSnapshot");
    await waitFor(() => observer.messages.slice(snapshotStart).some((message) => message.type === "StateSnapshot"),
      { label: "authoritative panel cursor after history keys" });
    if (observer.state.panels.left.cursor !== beforeCursor) throw new Error("history keys reached the covered panel");
    const levels = await browser.execute(() => Array.from(document.querySelectorAll('[data-testid="notification-entry"]')).map((row) => row.getAttribute("data-level")));
    if (JSON.stringify(levels) !== JSON.stringify(["error", "warning", "info", "info"])) throw new Error(`wrong history order/severity: ${levels}`);

    // Clearing history leaves active and queued delivery intact and creates no notification.
    await browser.performActions([{ type: "key", id: "keyboard", actions: [
      { type: "keyDown", value: "\uE008" }, { type: "keyDown", value: "\uE004" },
      { type: "keyUp", value: "\uE004" }, { type: "keyUp", value: "\uE008" },
    ] }]);
    await browser.releaseActions();
    await expectFocused(browser, '[data-testid="notifications-clear"]', "return to Clear history");
    await browser.keys("Enter");
    await waitFor(() => observer.state.notifications.entries.length === 0, { label: "burst history cleared" });
    await waitForToast(browser, "burst second");
    await waitForToast(browser, "burst third");
    await waitFor(() => browser.execute(() => !document.querySelector('[data-testid="transient-hint"]')),
      { timeout: 10_000, label: "all burst toasts expired" });
    if (observer.state.notifications.entries.length !== 0) throw new Error("clear created a recursive notification");

    // A burst taller than the viewport must keep the unchanged active row visible.
    await request(observer, "PublishNotification", { message: "overflow anchor", channels: [] });
    await waitFor(() => browser.execute(() => document.querySelector('[data-testid="notifications-list"]')?.textContent.includes("overflow anchor")),
      { label: "overflow anchor displayed" });
    const anchorId = await browser.execute(() => document.querySelector('[data-testid="notifications-list"]')?.getAttribute("aria-activedescendant"));
    for (let index = 0; index < 40; index += 1) {
      await request(observer, "PublishNotification", { message: `overflow arrival ${index}`, channels: [] });
    }
    await waitFor(() => browser.execute((anchorId) => {
      const list = document.querySelector('[data-testid="notifications-list"]');
      const row = document.getElementById(anchorId);
      if (!list || !row || list.getAttribute("aria-activedescendant") !== anchorId || list.scrollHeight <= list.clientHeight) return false;
      const bounds = list.getBoundingClientRect();
      const activeBounds = row.getBoundingClientRect();
      return activeBounds.top >= bounds.top && activeBounds.bottom <= bounds.bottom;
    }, anchorId), { label: "overflow arrivals keep the browsed row visible" });
    await expectFocused(browser, '[data-testid="notifications-list"]', "overflow arrivals preserve focus");
    await request(observer, "ClearNotifications");

    await request(observer, "PublishNotification", { message: "reload marker", channels: ["log"] });
    for (const scheme of ["daylight", "fbro-dark"]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      await waitFor(() => browser.execute((scheme) => document.querySelector(".app")?.getAttribute("data-color-scheme") === scheme, scheme),
        { label: `notification history in ${scheme}` });
      if (!(await (await browser.$('[data-testid="notification-entry"]')).getText()).includes("reload marker")) throw new Error("theme change lost history");
    }
    await browser.keys("Escape");
    await expectFocused(browser, '[data-testid="panel-left"]', "history close restores exact panel opener");

    await browser.refresh();
    await (await browser.$('[data-testid="panel-left"]')).waitForDisplayed({ timeout: 15_000 });
    await waitFor(() => browser.execute(() => !document.querySelector(".banner--disconnected")), { label: "frontend reconnected" });
    await openHistory(browser);
    await waitFor(() => browser.execute(() => document.querySelector('[data-testid="notifications-list"]')?.textContent.includes("reload marker")),
      { label: "backend history survives frontend reload" });
    if (await browser.execute(() => !!document.querySelector('[data-testid="transient-hint"]'))) throw new Error("history replayed as a toast on reload");
    await browser.keys("Escape");
  } finally {
    await browser.keys("Escape");
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    await goToPanelPath(browser, observer, "left", previousPath);
    fs.rmSync(fixture, { recursive: true, force: true });
  }
}
