import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickEntry, goToPanelPath } from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";
import { waitForToast } from "./notifications.mjs";

const panelSelector = '[data-testid="panel-left"]';

function replaceFileWithDirectory(hostDir) {
  fs.renameSync(path.join(hostDir, "a.txt"), path.join(hostDir, "empty", "saved-a.txt"));
  fs.mkdirSync(path.join(hostDir, "a.txt"));
}

function restoreFile(hostDir) {
  fs.rmdirSync(path.join(hostDir, "a.txt"));
  fs.renameSync(path.join(hostDir, "empty", "saved-a.txt"), path.join(hostDir, "a.txt"));
}

async function expectFocus(browser, selector, label) {
  await waitFor(() => browser.execute((selector) =>
    document.querySelector(selector) === document.activeElement, selector),
  { timeout: 5_000, label }).catch(async (error) => {
    const active = await browser.execute(() => ({
      tag: document.activeElement?.tagName, className: document.activeElement?.className,
      panel: document.activeElement?.closest("[data-panel-id]")?.getAttribute("data-panel-id"),
      modals: document.querySelectorAll("[data-modal-id]").length,
    }));
    throw new Error(`${error.message}; active=${JSON.stringify(active)}`);
  });
}

async function chord(browser, modifier, letter) {
  await browser.performActions([{
    type: "key", id: "keyboard", actions: [
      { type: "keyDown", value: modifier }, { type: "keyDown", value: letter },
      { type: "keyUp", value: letter }, { type: "keyUp", value: modifier },
    ],
  }]);
  await browser.releaseActions();
}

async function expectFirstArrow(browser, observer, panelId = "left") {
  const before = await browser.execute((panelId) => Number(document.querySelector(
    `[data-panel-id="${panelId}"] .panel__row--cursor`,
  )?.getAttribute("data-abs-index")), panelId);
  const key = before > 0 ? "ArrowUp" : "ArrowDown";
  // No focus() or Tab workaround between the handoff and this actual keypress.
  await browser.keys(key);
  await waitFor(() => {
    // Cursor patches are sent to the initiating UI connection, so ask the
    // observer for authoritative snapshots rather than inspecting stale state.
    observer.sendIntent("RequestStateSnapshot");
    return observer.state.panels[panelId].cursor === before + (key === "ArrowUp" ? -1 : 1);
  },
    { timeout: 5_000, label: `${panelId}: first arrow moves backend cursor` });
}

export async function runFocusHandoffFlow(browser, observer) {
  const previous = Object.fromEntries(["left", "right"].map((id) => [id, observer.state.panels[id].path]));
  const previousFocus = observer.state.focus;
  const hostDir = fs.mkdtempSync(hostPath(backendPath("a", "src", "focus-handoff-")));
  const dir = backendPath("a", "src", path.basename(hostDir));
  fs.writeFileSync(path.join(hostDir, "a.txt"), "unchanged\n");
  fs.writeFileSync(path.join(hostDir, "b.txt"), "second\n");
  fs.writeFileSync(path.join(hostDir, "c.txt"), "third\n");
  fs.mkdirSync(path.join(hostDir, "empty"));
  const hint = await browser.$('[data-testid="transient-hint"]');
  const bookmarks = await browser.$('[data-testid="bookmarks-popover"]');
  let completed = false;
  try {
    await goToPanelPath(browser, observer, "left", dir);
    await clickEntry(browser, "left", "a.txt");
    await browser.keys("s");
    await observer.waitForPanelPath("right", dir);
    await observer.waitForPanelReady("right");
    await browser.keys("F5");
    await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "self-copy confirmation");
    await browser.keys("Enter");
    await waitForToast(browser, "onto itself");
    if (!(await hint.getText()).includes("onto itself")) throw new Error("missing self-copy warning");
    await expectFocus(browser, panelSelector, "backend rejection returns panel focus");
    await hint.click();
    await expectFocus(browser, panelSelector, "toast click preserves panel focus");
    await expectFirstArrow(browser, observer);
    await hint.waitForDisplayed({ timeout: 5_000, reverse: true });
    await expectFirstArrow(browser, observer);
    if (fs.readFileSync(path.join(hostDir, "a.txt"), "utf8") !== "unchanged\n") {
      throw new Error("rejected self-copy changed the file");
    }

    await clickEntry(browser, "left", "a.txt");
    await chord(browser, "\uE008", "g");
    await waitForToast(browser, "Git repository actions");
    if (!(await hint.getText()).includes("Git repository actions")) throw new Error("missing non-repository hint");
    // The empty bookmark list also owns focus and must return it on Escape.
    await chord(browser, "\uE009", "d");
    await bookmarks.waitForDisplayed({ timeout: 5_000 });
    await browser.keys("Escape");
    await bookmarks.waitForDisplayed({ timeout: 5_000, reverse: true });
    await expectFocus(browser, panelSelector, "non-repository/bookmark cancellation handoff");
    await expectFirstArrow(browser, observer);

    observer.sendIntent("AddBookmark", undefined, { name: "focus handoff", path: dir });
    await waitFor(() => observer.state.bookmarks.some((bookmark) => bookmark.path === dir),
      { timeout: 5_000, label: "focus bookmark fixture" });
    await clickEntry(browser, "left", "a.txt");
    await chord(browser, "\uE009", "d");
    await bookmarks.waitForDisplayed({ timeout: 5_000 });
    const beforeBookmarkArrow = observer.state.panels.left.cursor;
    await browser.keys("ArrowDown");
    if (observer.state.panels.left.cursor !== beforeBookmarkArrow) throw new Error("bookmark arrow reached panel");
    await browser.keys("Enter");
    await bookmarks.waitForDisplayed({ timeout: 5_000, reverse: true });
    await expectFocus(browser, panelSelector, "bookmark navigation handoff");
    // Hover outside both listings: handoff must not depend on pointer position.
    await (await browser.$('[data-command-id="file.view"]')).moveTo();
    await expectFirstArrow(browser, observer);

    // A menu launched from an inline control restores that exact control.
    await clickEntry(browser, "left", "a.txt");
    await browser.keys("ArrowRight");
    const inlineSelector = `${panelSelector} [data-entry-name="a.txt"] [data-inline-operation="panel.pass-to-bro"]`;
    await expectFocus(browser, inlineSelector, "inline opener");
    await chord(browser, "\uE009", "d");
    await bookmarks.waitForDisplayed({ timeout: 5_000 });
    await browser.keys("Escape");
    await expectFocus(browser, inlineSelector, "bookmarks restores exact inline opener");
    await browser.keys("c");
    await (await browser.$(".context-menu")).waitForDisplayed({ timeout: 5_000 });
    await (await browser.$('.context-menu [data-command-id="file.rename"]')).click();
    await expectModalFocus(browser, '[data-testid="rename-to-input"]', "menu successor modal owns focus");
    await browser.keys("Escape");
    await expectFocus(browser, inlineSelector, "menu-to-modal chain restores original opener");

    // A listing refresh can remove choices without any keyboard navigation.
    await browser.keys("c");
    await (await browser.$(".context-menu")).waitForDisplayed({ timeout: 5_000 });
    await browser.keys("Escape");
    await expectFocus(browser, inlineSelector, "context menu cancellation handoff");
    replaceFileWithDirectory(hostDir);
    await observer.waitForEntry("left", "a.txt", "directory");
    await expectFocus(browser, panelSelector, "removed inline control hands focus to listing");
    await expectFirstArrow(browser, observer);
    restoreFile(hostDir);
    await observer.waitForEntry("left", "a.txt", "file");

    // Closing a modal falls back to the listing if its inline opener vanished.
    await clickEntry(browser, "left", "a.txt");
    const renameSelector = `${panelSelector} [data-entry-name="a.txt"] [data-inline-operation="file.rename"]`;
    await browser.execute((selector) => document.querySelector(selector)?.focus(), renameSelector);
    await browser.keys("Enter");
    await expectModalFocus(browser, '[data-testid="rename-to-input"]', "inline rename modal");
    replaceFileWithDirectory(hostDir);
    await observer.waitForEntry("left", "a.txt", "directory");
    await (await browser.$(renameSelector)).waitForExist({ timeout: 5_000, reverse: true });
    await expectModalFocus(browser, '[data-testid="rename-to-input"]', "removal preserves modal focus");
    await browser.keys("Escape");
    await expectFocus(browser, panelSelector, "modal missing-opener fallback");
    await expectFirstArrow(browser, observer);
    restoreFile(hostDir);
    await observer.waitForEntry("left", "a.txt", "file");

    // Toolbar mouse activation must leave the panel as the modal's opener.
    await (await browser.$('[data-command-id="file.rename"]')).click();
    await expectModalFocus(browser, '[data-testid="rename-to-input"]', "toolbar rename modal");
    await browser.keys("Escape");
    await expectFocus(browser, panelSelector, "toolbar modal returns to panel");

    await browser.keys("f");
    const filterSelector = `${panelSelector} .panel__filter-input`;
    await expectFocus(browser, filterSelector, "filter owns focus");
    await chord(browser, "\uE009", "d");
    await bookmarks.waitForDisplayed({ timeout: 5_000 });
    await browser.keys("Escape");
    await expectFocus(browser, filterSelector, "bookmarks returns to filter input");
    await browser.keys("Escape");
    await expectFocus(browser, panelSelector, "filter close returns to listing");

    // An intentional outside click takes ownership rather than being undone.
    await chord(browser, "\uE009", "d");
    await bookmarks.waitForDisplayed({ timeout: 5_000 });
    await clickEntry(browser, "right", "b.txt");
    await expectFocus(browser, '[data-testid="panel-right"]', "outside click preserves other panel focus");
    await expectFirstArrow(browser, observer, "right");

    await clickEntry(browser, "left", "a.txt");
    await browser.keys("c");
    await (await browser.$(".context-menu")).waitForDisplayed({ timeout: 5_000 });
    replaceFileWithDirectory(hostDir);
    await observer.waitForEntry("left", "a.txt", "directory");
    await (await browser.$(".context-menu")).waitForDisplayed({ timeout: 5_000, reverse: true });
    await expectFocus(browser, panelSelector, "automatic menu closure hands focus to listing");
    restoreFile(hostDir);
    await goToPanelPath(browser, observer, "left", `${dir}/empty`);
    await expectFocus(browser, `${panelSelector} [data-testid="empty-folder-new-file"]`, "navigation hands focus to empty-folder suggestion");
    await browser.keys("Enter");
    await expectModalFocus(browser, '[data-testid="touch-name-input"]', "suggestion owns native activation");
    await browser.keys("Escape");
    await expectFocus(browser, `${panelSelector} [data-testid="empty-folder-new-file"]`, "empty suggestion restored");

    await goToPanelPath(browser, observer, "left", dir);
    await clickEntry(browser, "left", "a.txt");
    await browser.keys("t");
    const terminalSelector = `${panelSelector} .xterm-helper-textarea`;
    await expectFocus(browser, terminalSelector, "terminal owns focus");
    await (await browser.$('[data-command-id="file.copy"]')).click();
    await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "copy modal opened from terminal");
    await browser.keys("Enter");
    await hint.waitForDisplayed({ timeout: 5_000 });
    await expectFocus(browser, terminalSelector, "backend rejection restores terminal opener");
    await hint.click();
    await expectFocus(browser, terminalSelector, "toast preserves terminal input");
    await (await browser.$(`${panelSelector} .panel-terminal__close`)).click();
    await waitFor(() => {
      observer.sendIntent("RequestStateSnapshot");
      return observer.state.panels.left.terminalSessionId === undefined;
    },
      { timeout: 5_000, label: "terminal closed by backend" });
    await expectFocus(browser, panelSelector, "terminal closure hands focus to listing");
    await expectFirstArrow(browser, observer);
    completed = true;
  } finally {
    observer.sendIntent("RemoveBookmark", undefined, { path: dir });
    // Clean up through backend navigation even if a failed assertion left a
    // modal open; cleanup must not replace the original failure.
    for (const id of ["left", "right"]) {
      if (completed) await goToPanelPath(browser, observer, id, previous[id]);
      else {
        observer.navigatePanelToPath(id, previous[id]);
        await observer.waitForPanelPath(id, previous[id]);
        await observer.waitForPanelReady(id);
      }
    }
    fs.rmSync(hostDir, { recursive: true, force: true });
    if (completed) {
      await (await browser.$("[data-modal-id]")).waitForExist({ timeout: 5_000, reverse: true });
      for (const id of ["left", "right"]) {
        await browser.execute((id) => document.querySelector(`[data-panel-id="${id}"]`)?.focus(), id);
        await waitFor(() => browser.execute((id) =>
          document.querySelector(`[data-panel-id="${id}"]`)?.classList.contains("panel--focused"), id),
        { timeout: 5_000, label: `${id}: restored listing active` });
        await browser.keys("r");
      }
      // Wait for the restored listing to observe fixture removal before the
      // next flow opens a modal. That empty-state transition owns initial focus.
      await waitFor(() => browser.execute((name) =>
        !Array.from(document.querySelectorAll("[data-entry-name]"))
          .some((entry) => entry.getAttribute("data-entry-name") === name)
        && !document.querySelector("[data-modal-id]"), path.basename(hostDir)),
      { timeout: 5_000, label: "focus fixture removed from restored listings" });
    }
    await browser.execute((id) => document.querySelector(`[data-panel-id="${id}"]`)?.focus(), previousFocus);
    if (completed) {
      await waitFor(() => browser.execute((id) =>
        document.querySelector(`[data-panel-id="${id}"]`)?.classList.contains("panel--focused"), previousFocus),
      { timeout: 5_000, label: "restore original backend focus" });
      await browser.execute(async (id) => {
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        document.querySelector(`[data-panel-id="${id}"]`)?.focus();
      }, previousFocus);
    }
  }
}
