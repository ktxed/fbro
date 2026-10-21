import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickEntry, goToPanelPath, setTextInputValue } from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus, expectCoveredModal } from "./modal-focus.mjs";

const input = '[data-testid="selection-mask-input"]';
const dialog = '[data-testid="selection-mask-dialog"]';

export async function runSelectionMaskFlow(browser, observer) {
  await freshPanel(observer);
  const restorePath = observer.state.panels.left.path;
  const restoreMode = observer.state.panels.left.viewMode;
  const restoreScheme = observer.state.settings.general.colorScheme;
  const directory = backendPath("a", "src", "selection-mask-smoke");
  fs.mkdirSync(hostPath(path.posix.join(directory, "folder.txt")), { recursive: true });
  for (const name of ["alpha.txt", "BETA.TXT", "photo.jpg", "ab.png", "README"]) {
    fs.writeFileSync(hostPath(path.posix.join(directory, name)), name);
  }
  await browser.execute(() => {
    window.__maskIntents = [];
    window.__maskOriginalSend = WebSocket.prototype.send;
    WebSocket.prototype.send = function(data) {
      if (typeof data === "string") {
        const message = JSON.parse(data);
        if (message.type === "Intent") {
          window.__maskSocket = this;
          if (["SelectByMask", "DeselectByMask"].includes(message.name)) window.__maskIntents.push(message);
        }
      }
      return window.__maskOriginalSend.call(this, data);
    };
  });
  try {
    for (const mode of ["list", "grid"]) {
      await goToPanelPath(browser, observer, "left", directory);
      await observer.waitForAllEntries("left", ["folder.txt", "alpha.txt", "BETA.TXT", "photo.jpg", "ab.png", "README"]);
      await freshPanel(observer);
      if (observer.state.panels.left.viewMode !== mode) {
        await browser.execute(() => document.querySelector('[data-testid="panel-left"]').focus());
        await browser.keys("g");
      }
      await waitFor(() => browser.execute(mode =>
        document.querySelector('[data-testid="panel-left"] .panel__list')?.classList.contains("panel__list--grid") === (mode === "grid"), mode),
      { label: `mask ${mode} view` });
      await guiIntent(browser, "SetGeneralOption", { key: "colorScheme", value: mode === "list" ? "fbro-dark" : "daylight" });
      await guiIntent(browser, "ClearSelection");
      await clickEntry(browser, "left", "folder.txt");
      await browser.keys("Space");
      await clickEntry(browser, "left", "photo.jpg");
      await browser.keys("Space");
      await clickEntry(browser, "left", "alpha.txt");

      await openMask(browser, "+", "*.txt", { code: "Equal", shiftKey: true });
      const before = await countIntents(browser);
      await browser.keys("Enter");
      await closed(browser);
      await expectSelection(observer, ["folder.txt", "photo.jpg", "alpha.txt", "BETA.TXT"]);
      assert.equal(await countIntents(browser), before + 1, "Enter must send one mask intent");
      await listingFocus(browser);
      await repeatShortcut(browser, "+", "Equal", true);

      await openMask(browser, "-", "*.txt", { code: "NumpadSubtract" });
      await browser.keys("Enter");
      await closed(browser);
      await expectSelection(observer, ["folder.txt", "photo.jpg"]);
      await listingFocus(browser);
      await repeatShortcut(browser, "-", "NumpadSubtract");

      await clickEntry(browser, "left", "README");
      await openMask(browser, "+", "*.*", { code: "NumpadAdd" });
      await browser.keys("*.TXT a?.png");
      assert.equal(await (await browser.$(input)).getValue(), "*.TXT a?.png", "typing must replace the default");
      await browser.keys("Enter");
      await closed(browser);
      await expectSelection(observer, ["folder.txt", "photo.jpg", "alpha.txt", "BETA.TXT", "ab.png"]);

      await clickEntry(browser, "left", "folder.txt");
      await openMask(browser, "+", "*.*");
      const cancelledBefore = await countIntents(browser);
      await setTextInputValue(browser, input, "  ");
      assert(await (await browser.$('[data-command-id="panel.selection.mask.confirm"]')).getAttribute("disabled") !== null,
        "blank masks disable confirmation");
      await browser.keys("Enter");
      assert.equal(await countIntents(browser), cancelledBefore, "empty masks must not submit");
      await setTextInputValue(browser, input, "*.*");
      await browser.keys("Tab");
      await browser.keys("Tab");
      assert.equal(await browser.execute(() => document.activeElement?.getAttribute("data-command-id")), "panel.selection.mask.cancel");
      await browser.keys("Enter");
      await closed(browser);
      assert.equal(await countIntents(browser), cancelledBefore, "Cancel Enter must not select");
      await listingFocus(browser);

      // Shift-produced + and unmodified - must remain text in the panel filter.
      await browser.keys("f");
      await waitFor(() => browser.execute(() => document.activeElement?.classList.contains("panel__filter-input")),
        { label: "filter owns mask characters" });
      await browser.keys("+-");
      assert.equal(await (await browser.$('[data-testid="panel-left"] .panel__filter-input')).getValue(), "+-");
      assert.equal(await (await browser.$(dialog)).isExisting(), false);
      await browser.keys("Escape");
      await listingFocus(browser);
      await waitFor(async () => {
        await freshPanel(observer);
        return observer.state.panels.left.status === "ready" && !observer.state.panels.left.filter;
      }, { label: "filter closed before mask" });

      await openMask(browser, "+", "*.*");
      // The top dialog owns these characters and arrows; covered panel keys are inert.
      await browser.keys("+-");
      assert.equal(await (await browser.$(input)).getValue(), "+-");
      await browser.execute(() => document.querySelector('[data-testid="panel-left"]').dispatchEvent(
        new KeyboardEvent("keydown", { key: "+", code: "NumpadAdd", bubbles: true, cancelable: true })));
      assert.equal(await browser.$$('[data-modal-id="selection-mask"]').then(layers => layers.length), 1);
      await browser.keys("Escape");
      await closed(browser);
      await listingFocus(browser);

      await openMask(browser, "+", "*.*");
      await browser.keys(["Alt", "ArrowDown"]);
      await browser.releaseActions();
      await browser.keys("Escape");
      assert.equal(await (await browser.$(dialog)).isDisplayed(), true, "first Escape closes input history");
      await browser.keys("Escape");
      await closed(browser);

      await openMask(browser, "+", "*.*");
      await browser.keys("Enter");
      await closed(browser);
      await expectSelection(observer, ["folder.txt", "alpha.txt", "BETA.TXT", "photo.jpg", "ab.png", "README"]);
      await listingFocus(browser);

      // An asynchronous dialog may cover this one; removal must restore its input.
      await openMask(browser, "-", "*.*");
      await browser.keys(["Alt", "ArrowDown"]);
      await browser.releaseActions();
      await waitFor(() => browser.execute(() => !!document.querySelector(".input-history")),
        { label: "mask history open before covering dialog" });
      await browser.execute(() => {
        window.__maskSocket.dispatchEvent(new MessageEvent("message", { data: JSON.stringify({
          type: "InputRequired", timestamp: Date.now(), panelId: "left", requestId: "mask-smoke", kind: "text", prompt: "Mask ownership test",
        }) }));
      });
      await expectModalFocus(browser, '[data-modal-id="input"] input', "covering input focus");
      await expectCoveredModal(browser, "selection-mask");
      await browser.keys("Escape");
      await expectModalFocus(browser, input, "uncovered mask input");
      await browser.keys("Escape");
      assert.equal(await (await browser.$(dialog)).isDisplayed(), true, "restored history closes before its mask dialog");
      await browser.keys("Escape");
      await closed(browser);
      await listingFocus(browser);

      // The palette handoff must retain input focus and select its initial mask.
      await browser.execute(() => document.activeElement.dispatchEvent(new KeyboardEvent("keydown", {
        key: "P", code: "KeyP", ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true,
      })));
      await expectModalFocus(browser, '[data-modal-id="command-palette"] input', "mask command palette");
      await browser.keys("Deselect Files");
      await browser.keys("Enter");
      await expectModalFocus(browser, input, "mask opened from palette");
      await browser.keys("Escape");
      await closed(browser);
      await listingFocus(browser);
    }

    const fontSize = await browser.execute(() => Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")));
    await guiIntent(browser, "SetFontSize", { value: 13 });
    await waitFontSize(browser, 13);
    for (const [key, expected] of [["+", 14], ["-", 13]]) {
      await browser.execute(key => document.activeElement.dispatchEvent(new KeyboardEvent("keydown", {
        key, ctrlKey: true, bubbles: true, cancelable: true,
      })), key);
      await waitFontSize(browser, expected);
      assert.equal(await (await browser.$(dialog)).isExisting(), false, "Ctrl zoom must not open a mask dialog");
    }
    await guiIntent(browser, "SetFontSize", { value: fontSize });

    const large = backendPath("a", "src", "selection-mask-windowed");
    fs.mkdirSync(hostPath(large), { recursive: true });
    for (let index = 0; index < 3201; index += 1) {
      fs.writeFileSync(hostPath(path.posix.join(large, `file-${String(index).padStart(4, "0")}.txt`)), "");
    }
    await goToPanelPath(browser, observer, "left", large);
    await observer.waitForPanelReady("left");
    await freshPanel(observer);
    if (observer.state.panels.left.viewMode === "grid") {
      await browser.execute(() => document.querySelector('[data-testid="panel-left"]').focus());
      await browser.keys("g");
    }
    await waitFor(() => browser.execute(() => {
      const list = document.querySelector('[data-testid="panel-left"] .panel__list');
      return list && !list.classList.contains("panel__list--grid") && !!list.querySelector(".panel__virtual-sizer");
    }), { label: "large listing uses Mode B in list view" });
    await browser.keys("Home");
    await browser.execute(async () => {
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      const list = document.querySelector('[data-testid="panel-left"] .panel__list');
      list.scrollTop = list.scrollHeight;
      list.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    // Observer snapshots deliberately contain full listings, without window
    // metadata. Inspect the GUI's actual render window for this assertion.
    await waitFor(() => browser.execute(() => {
      const panel = document.querySelector('[data-testid="panel-left"]');
      return !panel.querySelector('[data-abs-index="0"]') && !!panel.querySelector('[data-abs-index="3200"]');
    }), { label: "cursor entry unloaded by virtual scrolling" });
    await openMask(browser, "+", "*.txt");
    await setTextInputValue(browser, input, "file-3200.*");
    await browser.keys("Enter");
    await closed(browser);
    await expectSelection(observer, ["file-3200.txt"]);
    await openMask(browser, "+", "*.txt");
    await setTextInputValue(browser, input, "*.*");
    await browser.keys("Enter");
    await closed(browser);
    await waitFor(async () => {
      await freshPanel(observer);
      return observer.state.panels.left.selection.length === 3201;
    }, { label: "mask selects full backend listing" });
    const intents = await browser.execute(() => window.__maskIntents);
    assert(intents.every(intent => intent.scope === "left"), "mask intents use the captured panel id");
  } catch (error) {
    console.error("Selection-mask assertion failed:", error);
    throw error;
  } finally {
    // Preserve the original assertion when failure leaves a prompt open.
    for (let attempt = 0; attempt < 3; attempt += 1) {
      if (!await browser.execute(() => !!document.querySelector('[data-modal-state="top"]'))) break;
      await browser.keys("Escape");
    }
    await guiIntent(browser, "SetGeneralOption", { key: "colorScheme", value: restoreScheme });
    await browser.execute(() => {
      WebSocket.prototype.send = window.__maskOriginalSend;
      delete window.__maskOriginalSend;
      delete window.__maskSocket;
      delete window.__maskIntents;
    });
    await goToPanelPath(browser, observer, "left", restorePath);
    await freshPanel(observer);
    if (observer.state.panels.left.viewMode !== restoreMode) {
      await browser.execute(() => document.querySelector('[data-testid="panel-left"]').focus());
      await browser.keys("g");
    }
  }
}

async function openMask(browser, key, expected, { code = key === "+" ? "NumpadAdd" : "Minus", shiftKey = false } = {}) {
  await browser.execute((key, code, shiftKey) => {
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key, code, shiftKey, bubbles: true, cancelable: true }));
  }, key, code, shiftKey);
  await expectModalFocus(browser, input, "mask input focus");
  await waitFor(async () => await (await browser.$(input)).getValue() === expected, { label: `mask prefill ${expected}` });
  const selection = await browser.execute(() => {
    const input = document.querySelector('[data-testid="selection-mask-input"]');
    return [input.selectionStart, input.selectionEnd, input.value.length];
  });
  assert.deepEqual(selection, [0, expected.length, expected.length], "prefill must be selected");
}

async function repeatShortcut(browser, key, code, shiftKey = false) {
  const consumed = await browser.execute((key, code, shiftKey) => {
    const event = new KeyboardEvent("keydown", { key, code, shiftKey, repeat: true, bubbles: true, cancelable: true });
    document.activeElement.dispatchEvent(event);
    return event.defaultPrevented;
  }, key, code, shiftKey);
  assert(consumed, "held mask shortcuts must be consumed");
  assert.equal(await (await browser.$(dialog)).isExisting(), false, "held keys must not reopen the dialog");
}

async function guiIntent(browser, name, args = {}) {
  await browser.execute((name, args) => window.__maskSocket.send(JSON.stringify({ type: "Intent", timestamp: Date.now(), name, scope: "left", args })), name, args);
}

async function countIntents(browser) { return browser.execute(() => window.__maskIntents.length); }
async function waitFontSize(browser, expected) {
  await waitFor(() => browser.execute(expected => Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")) === expected, expected),
    { label: `mask preserves font zoom ${expected}` });
}
async function closed(browser) { await (await browser.$(dialog)).waitForExist({ reverse: true }); }
async function listingFocus(browser) {
  await waitFor(() => browser.execute(() => document.activeElement?.getAttribute("data-testid") === "panel-left"), { label: "mask returns listing focus" });
}
async function freshPanel(observer) {
  const from = observer.messages.length;
  observer.sendIntent("RequestStateSnapshot");
  await waitFor(() => observer.messages.slice(from).some(message => message.type === "StateSnapshot"), { label: "fresh mask panel snapshot" });
}
async function expectSelection(observer, names) {
  await waitFor(async () => {
    await freshPanel(observer);
    const panel = observer.state.panels.left;
    return panel.selection.length === names.length
      && JSON.stringify(panel.selectedEntries.map(entry => entry.name).sort()) === JSON.stringify([...names].sort());
  }, { label: `mask selection [${names.join(", ")}]` });
}
