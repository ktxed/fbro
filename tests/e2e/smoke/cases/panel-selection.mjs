import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickEntry, goToPanelPath } from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

// Changing Space back to size-only on the first folder breaks this flow.
// The small fixtures keep size assertions exact without timing a running scan.
export async function runPanelSelectionFlow(browser, observer) {
  // Prior GUI actions do not broadcast patches to this observer connection.
  // Capture fresh backend state so cleanup restores the actual opener mode.
  const snapshotStart = observer.messages.length;
  observer.sendIntent("RequestStateSnapshot");
  await waitFor(() => observer.messages.slice(snapshotStart).some(message => message.type === "StateSnapshot"),
    { label: "selection smoke initial panel state" });
  const restorePath = observer.state.panels.left.path;
  const restoreMode = observer.state.panels.left.viewMode;
  await browser.execute(() => {
    window.__panelSelectionIntents = [];
    window.__panelSelectionOriginalSend = WebSocket.prototype.send;
    WebSocket.prototype.send = function(data) {
      if (typeof data === "string") {
        const message = JSON.parse(data);
        if (message.type === "Intent" && ["ToggleSelection", "ToggleDirectorySizes", "CalculateSize", "CancelSizeCalculation"].includes(message.name)) {
          window.__panelSelectionIntents.push(message);
        }
      }
      return window.__panelSelectionOriginalSend.call(this, data);
    };
  });
  try {
    for (const mode of ["list", "grid"]) {
      const directory = backendPath("a", "src", "panel-selection-smoke", mode);
      for (const name of ["alpha", "beta", "zero"]) {
        fs.mkdirSync(hostPath(path.posix.join(directory, name)), { recursive: true });
      }
      fs.writeFileSync(hostPath(path.posix.join(directory, "alpha", "payload.txt")), "alpha bytes");
      fs.writeFileSync(hostPath(path.posix.join(directory, "beta", "payload.txt")), "beta");
      fs.writeFileSync(hostPath(path.posix.join(directory, "file.txt")), "file");
      await goToPanelPath(browser, observer, "left", directory);
      await observer.waitForAllEntries("left", ["alpha", "beta", "zero", "file.txt"]);
      await setViewMode(browser, observer, mode);
      await clearSelection(observer);

      await clickEntry(browser, "left", "alpha");
      const cursor = observer.state.panels.left.cursor;
      await browser.keys("Space");
      await expectSelection(observer, ["alpha"]);
      await expectSize(observer, "alpha", 11);
      await expectListingFocus(browser);
      if (observer.state.panels.left.cursor !== cursor) throw new Error(`${mode}: Space moved the folder cursor`);

      await clickEntry(browser, "left", "file.txt");
      await browser.keys("Space");
      await expectSelection(observer, ["alpha", "file.txt"]);
      await browser.keys("Space");
      await expectSelection(observer, ["alpha"]);

      // The selected folder is the copy target even though the cursor is a file.
      await browser.keys("F5");
      await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "selected folder copy focus");
      const summary = await browser.$('[data-testid="operation-dialog"] .modal__summary strong');
      if (await summary.getText() !== "alpha") throw new Error(`${mode}: Copy did not target the selected folder`);
      const intentCount = await selectionIntentCount(browser);
      await browser.execute(() => {
        document.querySelector('[data-testid="panel-left"]').dispatchEvent(new KeyboardEvent("keydown", {
          key: " ", code: "Space", bubbles: true, cancelable: true,
        }));
      });
      await browser.execute(() => document.querySelector('[data-testid="copy-destination-name-input"]').focus());
      await browser.keys("Space");
      if (!(await browser.$('[data-testid="copy-destination-name-input"]').getValue()).includes(" ")) {
        throw new Error(`${mode}: copy input did not own Space`);
      }
      if (await selectionIntentCount(browser) !== intentCount) throw new Error(`${mode}: covered panel/input sent a Space intent`);
      await browser.execute(() => document.querySelector('[data-command-id="file.copy.cancel"]').focus());
      await browser.keys("Space");
      await browser.$('[data-testid="operation-dialog"]').waitForDisplayed({ timeout: 10_000, reverse: true });
      await expectListingFocus(browser);
      await expectSelection(observer, ["alpha"]);

      await clickEntry(browser, "left", "alpha");
      await verifyRepeatedSpace(browser);
      fs.writeFileSync(hostPath(path.posix.join(directory, "alpha", "payload.txt")), "updated");
      await browser.keys("Space");
      await expectSelection(observer, []);
      await browser.keys("Space");
      await expectSelection(observer, ["alpha"]);
      await expectSize(observer, "alpha", 11);

      await shiftSpace(browser);
      await expectSize(observer, "alpha", 7);
      await expectSelection(observer, ["alpha"]);
      if (observer.state.panels.left.entries.find(entry => entry.name === "beta").size != null) {
        throw new Error(`${mode}: selected-folder Shift+Space also sized unselected beta`);
      }
      await clearSelection(observer);
      await shiftSpace(browser);
      await expectSize(observer, "beta", 4);
      await expectSize(observer, "zero", 0);
      await expectSelection(observer, []);
      await expectListingFocus(browser);

      // Zero is a known size, so selecting an empty folder must reuse it too.
      fs.writeFileSync(hostPath(path.posix.join(directory, "zero", "payload.txt")), "filled");
      await clickEntry(browser, "left", "zero");
      await browser.keys("Space");
      await expectSelection(observer, ["zero"]);
      await expectSize(observer, "zero", 0);
      await shiftSpace(browser);
      await expectSize(observer, "zero", 6);
      await expectSelection(observer, ["zero"]);
      await clearSelection(observer);
    }
    const intents = await browser.execute(() => window.__panelSelectionIntents);
    if (!intents.some(intent => intent.name === "ToggleSelection" && intent.scope === "left" && intent.args.calculateSize === true)
      || !intents.some(intent => intent.name === "ToggleDirectorySizes" && intent.scope === "left")) {
      throw new Error(`Space commands did not target the resolved panel: ${JSON.stringify(intents)}`);
    }
  } finally {
    await browser.execute(() => {
      WebSocket.prototype.send = window.__panelSelectionOriginalSend;
      delete window.__panelSelectionOriginalSend;
      delete window.__panelSelectionIntents;
    });
    await goToPanelPath(browser, observer, "left", restorePath);
    await setViewMode(browser, observer, restoreMode);
  }
}

async function setViewMode(browser, observer, mode) {
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]').focus());
  if (observer.state.panels.left.viewMode !== mode) await browser.keys("g");
  await waitFor(() => browser.execute(mode => {
    const list = document.querySelector('[data-testid="panel-left"] .panel__list');
    return !!list && list.classList.contains("panel__list--grid") === (mode === "grid");
  }, mode), { label: `Space coverage in ${mode} view` });
}

async function clearSelection(observer) {
  observer.sendIntent("ClearSelection", "left");
  await expectSelection(observer, []);
}

async function expectSelection(observer, names) {
  const from = observer.messages.length;
  let lastSnapshotRequest = 0;
  await waitFor(() => {
    if (Date.now() - lastSnapshotRequest > 500) {
      lastSnapshotRequest = Date.now();
      observer.sendIntent("RequestStateSnapshot");
    }
    if (!observer.messages.slice(from).some(message => message.type === "StateSnapshot")) return false;
    const panel = observer.state.panels.left;
    const selected = (panel.selectedEntries ?? []).map(entry => entry.name).sort();
    return JSON.stringify(selected) === JSON.stringify([...names].sort()) && panel.selection.length === names.length;
  }, { label: `Space selection [${names.join(", ")}]` });
}

async function expectSize(observer, name, size) {
  const from = observer.messages.length;
  let lastSnapshotRequest = 0;
  await waitFor(() => {
    // GUI intents reply to the GUI connection. Read fresh backend snapshots
    // through the observer to synchronize selection and size assertions.
    if (Date.now() - lastSnapshotRequest > 500) {
      lastSnapshotRequest = Date.now();
      observer.sendIntent("RequestStateSnapshot");
    }
    if (!observer.messages.slice(from).some(message => message.type === "StateSnapshot")) return false;
    const panel = observer.state.panels.left;
    return panel.entries.find(entry => entry.name === name)?.size === size && !panel.calculatingSize.includes(name);
  }, { label: `calculated ${name} size ${size}` });
}

async function expectListingFocus(browser) {
  await waitFor(() => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"),
    { label: "Space preserves listing focus" });
}

async function selectionIntentCount(browser) {
  return browser.execute(() => window.__panelSelectionIntents.length);
}

async function verifyRepeatedSpace(browser) {
  const result = await browser.execute(() => {
    const before = window.__panelSelectionIntents.length;
    const consumed = [false, true].map(shiftKey => {
      const event = new KeyboardEvent("keydown", { key: " ", code: "Space", shiftKey, repeat: true, bubbles: true, cancelable: true });
      document.activeElement.dispatchEvent(event);
      return event.defaultPrevented;
    });
    return { consumed, sent: window.__panelSelectionIntents.length - before };
  });
  if (!result.consumed.every(Boolean) || result.sent !== 0) throw new Error(`Held Space/Shift+Space executed another action: ${JSON.stringify(result)}`);
}

async function shiftSpace(browser) {
  await browser.performActions([{
    type: "key", id: "keyboard", actions: [
      { type: "keyDown", value: "\uE008" },
      { type: "keyDown", value: " " },
      { type: "keyUp", value: " " },
      { type: "keyUp", value: "\uE008" },
    ],
  }]);
  await browser.releaseActions();
}
