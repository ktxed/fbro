// The terminal's floating action toolbar must never cover terminal output: it
// reveals on open, fades out, and comes back when the pointer approaches the
// corner. Because the fade takes the buttons out of reach, maximizing and
// promoting are also registry commands, usable while the terminal owns the keys.
import assert from "node:assert/strict";

import { goToPanelPath, movePointerTo, revealTerminalToolbar } from "../browser.mjs";
import { backendPath } from "../config.mjs";
import { waitFor } from "../wait.mjs";

const W3C = { ctrl: "\uE009", shift: "\uE008" };
/** Must stay under the app's TOOLBAR_IDLE_HIDE_MS so the reveal assertions are
 *  about the reveal, not about a fade that already happened. */
const REVEAL_ASSERT_WINDOW_MS = 1_000;

async function chord(browser, modifiers, key) {
  await browser.performActions([{
    type: "key",
    id: "terminal-toolbar-keyboard",
    actions: [
      ...modifiers.map((value) => ({ type: "keyDown", value })),
      { type: "keyDown", value: key },
      { type: "keyUp", value: key },
      ...modifiers.slice().reverse().map((value) => ({ type: "keyUp", value })),
    ],
  }]);
  await browser.releaseActions();
}

function toolbarStyle(browser, selector) {
  return browser.execute((selector) => {
    const toolbar = document.querySelector(selector);
    if (!toolbar) return null;
    const style = getComputedStyle(toolbar);
    return {
      revealed: toolbar.classList.contains("panel-terminal__toolbar--revealed"),
      opacity: style.opacity,
      visibility: style.visibility,
      pointerEvents: style.pointerEvents,
    };
  }, selector);
}

function expectToolbar(browser, selector, expected, label, timeout = 8_000) {
  const want = Object.entries(expected).map(([key, value]) => `${key}=${value}`).join(" ");
  return waitFor(async () => {
    const style = await toolbarStyle(browser, selector);
    return style && Object.entries(expected).every(([key, value]) => style[key] === value);
  }, { timeout, label: `${label}: toolbar ${want}` });
}

function expectFocused(browser, selector, label) {
  return waitFor(() => browser.execute((selector) => {
    const active = document.activeElement;
    return !!active && active.matches(selector);
  }, selector), { timeout: 10_000, label: `${label}: focus on ${selector}` });
}

export async function runTerminalToolbarFlow(browser, observer) {
  const panelId = "left";
  const previousPath = observer.state.panels[panelId]?.path;
  const panelSelector = `[data-testid="panel-${panelId}"]`;
  const toolbar = `${panelSelector} [data-testid="terminal-toolbar"]`;
  const xtermInput = `${panelSelector} .xterm-helper-textarea`;
  const faded = { revealed: false, opacity: "0", visibility: "hidden", pointerEvents: "none" };
  const shown = { revealed: true, opacity: "1", visibility: "visible", pointerEvents: "auto" };
  try {
    await goToPanelPath(browser, observer, panelId, backendPath("a", "src"));
    await browser.keys("t");
    const xterm = await browser.$(`${panelSelector} .xterm`);
    await xterm.waitForDisplayed({ timeout: 10_000 });
    await expectFocused(browser, xtermInput, "the opened terminal owns the keyboard");
    const sessionId = observer.state.panels[panelId]?.terminalSessionId;
    assert.ok(sessionId !== undefined, "terminal session id from the backend");

    // Open: discoverable for the idle window, so the buttons are up on arrival.
    await expectToolbar(browser, toolbar, shown, "the toolbar is revealed when the terminal opens", 2_000);
    // Faded: invisible, unclickable, out of the tab order and the a11y tree.
    await expectToolbar(browser, toolbar, faded, "the toolbar fades out after the idle window");
    const fadedButton = await browser.$(`${panelSelector} .panel-terminal__close`);
    assert.ok(!(await fadedButton.isClickable()), "the faded close button is not pointer-interactable");

    // Approach: a real pointer move into the corner region brings it back, and
    // nothing about hovering moves focus away from the terminal.
    await revealTerminalToolbar(browser, panelSelector);
    await expectToolbar(browser, toolbar, shown, "pointer approach reveals the toolbar", REVEAL_ASSERT_WINDOW_MS);
    await expectFocused(browser, xtermInput, "hovering the corner preserves terminal focus");
    await (await browser.$(`${panelSelector} .panel-terminal__close`)).waitForClickable({ timeout: 5_000 });

    // Inside the terminal but away from the corner it fades out again.
    const away = await browser.execute((selector) => {
      const box = document.querySelector(selector).getBoundingClientRect();
      return { x: Math.round(box.left + 12), y: Math.round(box.bottom - 12) };
    }, `${panelSelector} .panel-terminal`);
    await movePointerTo(browser, away.x, away.y);
    await expectToolbar(browser, toolbar, faded, "the toolbar fades out once the pointer leaves the corner");

    // Keyboard: the registry chords work while the terminal owns the keys. The
    // PTY never sees them, so `Ctrl+M` keeps meaning a real carriage return.
    await chord(browser, [W3C.ctrl, W3C.shift], "m");
    await waitFor(() => browser.execute(() => !!document.querySelector(".panel-terminal--maximized")),
      { timeout: 5_000, label: "Ctrl+Shift+M maximizes the terminal from inside it" });
    await expectFocused(browser, xtermInput, "maximizing keeps the terminal focused");
    // Maximized, the corner is the window corner and nothing can be approached
    // from, so the reveal re-arms to keep the restore control reachable.
    await expectToolbar(browser, '[data-testid="terminal-toolbar"]', shown,
      "maximizing reveals the faded toolbar again", REVEAL_ASSERT_WINDOW_MS);

    await browser.keys("Escape");
    await waitFor(() => browser.execute(() => !document.querySelector(".panel-terminal--maximized")),
      { timeout: 5_000, label: "Escape restores the maximized terminal first" });
    await expectFocused(browser, xtermInput, "Escape stays in the terminal while restoring");

    // Promote: same live session, now in a tab of its own.
    await chord(browser, [W3C.ctrl, W3C.shift], "t");
    const promotedPanelId = await waitFor(() => {
      observer.sendIntent("RequestStateSnapshot");
      const slot = observer.state.slots.left;
      const active = observer.state.panels[slot.activePanelId];
      return slot.activePanelId !== panelId
        && active?.tabKind === "terminal"
        && active?.terminalSessionId === sessionId
        ? slot.activePanelId
        : false;
    }, { timeout: 10_000, label: "Ctrl+Shift+T promotes the terminal into its own tab with the same session" });

    // The tab switch remounts the component, which re-arms the grace window.
    const soloToolbar = ".panel-terminal--solo [data-testid=\"terminal-toolbar\"]";
    await expectToolbar(browser, soloToolbar, shown, "the solo terminal tab reveals its toolbar", REVEAL_ASSERT_WINDOW_MS);
    assert.ok(!(await browser.$(`${panelSelector} [data-testid="terminal-toolbar"]`).isExisting()),
      "the embedded split-view terminal is gone after promoting");

    // Close the promoted tab through the revealed toolbar (×), the mouse path.
    await revealTerminalToolbar(browser, "");
    await (await browser.$(".panel-terminal--solo .panel-terminal__close")).waitForClickable({ timeout: 5_000 });
    await (await browser.$(".panel-terminal--solo .panel-terminal__close")).click();
    await waitFor(() => {
      observer.sendIntent("RequestStateSnapshot");
      const slot = observer.state.slots.left;
      return slot.activePanelId === panelId && observer.state.panels[promotedPanelId]?.terminalSessionId === undefined;
    }, { timeout: 10_000, label: "the promoted terminal tab closed" });
    // Closing the listing-bearing tab brings the files tab back, and the mouse
    // path through the revealed toolbar was the way to get there.
    assert.equal(observer.state.panels[panelId]?.terminalSessionId, undefined, "no terminal left in the files tab");
  } finally {
    if (previousPath) await goToPanelPath(browser, observer, panelId, previousPath).catch(() => {});
  }
}
