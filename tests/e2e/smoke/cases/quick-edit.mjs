import fs from "node:fs";
import path from "node:path";

import { clickEntry, goToPanelPath } from "../browser.mjs";
import { backendPath, hostPath } from "../config.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

export async function runQuickEditFlow(browser, observer) {
  const directory = backendPath("a", "src");
  const target = path.posix.join(directory, "quick-edit-smoke.txt");
  const hostTarget = hostPath(target);
  const cursorName = "quick-edit-cursor.txt";
  const cursorTarget = hostPath(path.posix.join(directory, cursorName));
  const cursorContents = "leave the cursor file unchanged\n";
  fs.writeFileSync(hostTarget, "before\n");
  fs.writeFileSync(cursorTarget, cursorContents);

  await goToPanelPath(browser, observer, "left", directory);
  await observer.waitForEntry("left", "quick-edit-smoke.txt", "file");
  await observer.waitForEntry("left", cursorName, "file");
  await clickEntry(browser, "left", cursorName);
  // Explicit arguments win even when the backend cursor points to another file.
  observer.sendIntent("QuickEditFile", "left", { path: directory, filename: "quick-edit-smoke.txt" });

  let terminalOpened = false;
  await waitFor(() => {
    if (observer.state.panels.left?.terminalSessionId != null) terminalOpened = true;
    const saved = fs.readFileSync(hostTarget, "utf8").includes("quick edit saved");
    return terminalOpened && saved && observer.state.panels.left?.terminalSessionId == null;
  }, { timeout: 10_000, label: "Quick Edit terminal completion" });
  if (fs.readFileSync(cursorTarget, "utf8") !== cursorContents) {
    throw new Error("Quick Edit modified the cursor file instead of only its explicit target");
  }
  await expectListingFocus(browser, cursorName);

  // Create a genuinely zero-byte file through the empty-folder suggestion,
  // then invoke the same Quick Edit path without an intervening row click.
  const emptyDirectory = path.posix.join(directory, "quick-edit-empty");
  fs.mkdirSync(hostPath(emptyDirectory), { recursive: true });
  await goToPanelPath(browser, observer, "left", emptyDirectory);
  await waitFor(() => browser.execute(() =>
    document.activeElement?.getAttribute("data-testid") === "empty-folder-new-file"),
  { timeout: 10_000, label: "new-file suggestion before Quick Edit" });
  await browser.keys("Enter");
  await expectModalFocus(browser, '[data-testid="touch-name-input"]', "new empty file name focus");
  await (await browser.$('[data-testid="touch-name-input"]')).setValue("new.txt");
  await browser.keys("Enter");
  await observer.waitForEntry("left", "new.txt", "file");
  await observer.waitForPanelReady("left");
  await waitFor(() => browser.execute(() =>
    document.activeElement === document.querySelector('[data-testid="panel-left"]')
      && Boolean(document.querySelector('[data-testid="panel-left"] [data-entry-name="new.txt"]'))),
  { timeout: 10_000, label: "newly created file listing and focus" });
  const emptyTarget = hostPath(path.posix.join(emptyDirectory, "new.txt"));
  if (fs.statSync(emptyTarget).size !== 0) throw new Error("New file was not zero bytes");
  await browser.execute(() => {
    window.__quickEditIntents = [];
    window.__quickEditOriginalSend = WebSocket.prototype.send;
    WebSocket.prototype.send = function(data) {
      if (typeof data === "string") {
        const message = JSON.parse(data);
        if (message.type === "Intent" && message.name === "QuickEditFile") {
          window.__quickEditIntents.push(message);
        }
      }
      return window.__quickEditOriginalSend.call(this, data);
    };
  });
  try {
    // Ctrl+F4 is the Linux smoke fixture's Quick Edit binding.
    await browser.performActions([{
      type: "key", id: "quick-edit-keyboard", actions: [
        { type: "keyDown", value: "\uE009" },
        { type: "keyDown", value: "\uE034" },
        { type: "keyUp", value: "\uE034" },
        { type: "keyUp", value: "\uE009" },
      ],
    }]);
    await browser.releaseActions();
    const intents = await browser.execute(() => window.__quickEditIntents);
    if (intents.length !== 1 || intents[0].scope !== "left"
      || intents[0].args?.path !== emptyDirectory || intents[0].args?.filename !== "new.txt") {
      throw new Error(`Quick Edit did not send its displayed target: ${JSON.stringify(intents)}`);
    }
  } finally {
    await browser.execute(() => {
      WebSocket.prototype.send = window.__quickEditOriginalSend;
      delete window.__quickEditOriginalSend;
      delete window.__quickEditIntents;
    });
  }
  await waitFor(() => {
    observer.sendIntent("RequestStateSnapshot");
    // Saving proves the editor ran; don't require polling to catch its short-lived session.
    return fs.readFileSync(emptyTarget, "utf8").includes("quick edit saved")
      && observer.state.panels.left?.terminalSessionId == null;
  }, { timeout: 10_000, label: "Quick Edit newly created zero-byte file completion" });
  await expectListingFocus(browser, "new.txt");

  observer.sendIntent("QuickEditFile", "missing-panel", { path: emptyDirectory, filename: "new.txt" });
  observer.sendIntent("NavigateToSource", "left", { sourceId: "journal" });
  await waitFor(() => {
    const panel = observer.state.panels.left;
    if (panel?.sourceId !== "journal" || panel.status !== "ready") return false;
    const names = panel.entries.map((entry) => entry.name);
    return ["quick-edit_v1_intent_", "quick-edit_v1_completed_", "quick-edit_v1_failed_"]
      .every((prefix) => names.some((name) => name.startsWith(prefix)));
  }, { timeout: 10_000, label: "Quick Edit journal events" });

  const sourceId = observer.sourceIdForPath(directory);
  if (!sourceId) throw new Error(`No local source contains ${directory}`);
  observer.sendIntent("NavigateToSource", "left", { sourceId });
  await waitFor(() => observer.state.panels.left?.sourceId === sourceId, {
    timeout: 10_000,
    label: "restore source after Quick Edit journal assertions",
  });
  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: directory });
  await observer.waitForPanelPath("left", directory);
  await observer.waitForPanelReady("left");
}

async function expectListingFocus(browser, filename) {
  await waitFor(() => browser.execute((filename) => {
    const panel = document.querySelector('[data-testid="panel-left"]');
    const cursor = panel?.querySelector(".panel__row--cursor, .panel__card--cursor");
    return document.activeElement === panel && cursor?.getAttribute("data-entry-name") === filename;
  }, filename), { timeout: 10_000, label: `Quick Edit restores listing focus on ${filename}` });
}
