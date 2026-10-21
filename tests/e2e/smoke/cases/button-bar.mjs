import fs from "node:fs";
import { backendPath, hostPath } from "../config.mjs";
import { FbroObserver } from "../observer.mjs";
import { pressWindowKey, reloadBrowserApp, expectCheckboxSelected, goToPanelPath } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runButtonBarFlow(browser, observer) {
  const previous = observer.state.settings.general.showButtonBar;
  const previousScheme = observer.state.settings.general.colorScheme;
  const previousPath = observer.state.panels.left.path;
  const fixturePath = backendPath("a", "button-bar-fixture");
  const fixture = hostPath(fixturePath);
  const peer = new FbroObserver("button-bar-peer", observer.url);
  const isMac = await browser.execute(() => navigator.platform.startsWith("Mac"));
  const modifier = isMac ? { meta: true } : { ctrl: true };
  const panel = '[data-testid="panel-left"]';
  const checkbox = '[data-testid="show-button-bar"]';
  const waitVisible = (visible) => waitFor(async () =>
    observer.state.settings.general.showButtonBar === visible
      && peer.state.settings.general.showButtonBar === visible
      && await browser.execute((visible) => !!document.querySelector(".toolbar") === visible, visible),
  { timeout: 10_000, label: `button bar ${visible ? "shown" : "hidden"} in both clients` });
  const expectPanelFocus = () => waitFor(() => browser.execute((selector) =>
    document.activeElement === document.querySelector(selector), panel),
  { timeout: 5000, label: "button bar toggle preserves panel opener" });

  try {
    fs.mkdirSync(fixture, { recursive: true });
    fs.writeFileSync(`${fixture}/example.txt`, "Button bar keyboard fixture\n");
    await goToPanelPath(browser, observer, "left", fixturePath);
    await observer.waitForEntry("left", "example.txt", "file");
    await peer.connect();
    observer.sendIntent("SetGeneralOption", undefined, { key: "showButtonBar", value: true });
    await waitVisible(true);

    for (const scheme of ["fbro-dark", "daylight"]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      await waitFor(() => browser.execute((scheme) => document.querySelector(".app")?.dataset.colorScheme === scheme, scheme),
        { label: `button bar ${scheme} scheme` });
      await browser.execute((selector) => document.querySelector(selector)?.focus(), panel);
      const shownHeight = await browser.execute(() => document.querySelector(".layout").getBoundingClientRect().height);
      const barHeight = await browser.execute(() => document.querySelector(".toolbar").getBoundingClientRect().height);

      await pressWindowKey(browser, ",", modifier);
      await browser.$('[data-testid="settings-tab-general"]').waitForDisplayed();
      await browser.keys("ArrowDown");
      await browser.$(checkbox).waitForDisplayed();
      // The checkbox is the first Display control, reachable from its sidebar tab.
      await browser.keys("Tab");
      if (!await browser.execute((selector) => document.activeElement === document.querySelector(selector), checkbox)) {
        throw new Error("Display Tab did not reach the button bar checkbox");
      }
      await browser.keys(" ");
      await waitVisible(false);
      await expectCheckboxSelected(browser, checkbox, false, "button bar checkbox");
      await browser.keys(isMac ? "7" : "F7");
      if (await browser.$('[data-modal-id="mkdir"]').isExisting()) {
        throw new Error("Hidden bar handled a file shortcut through Settings");
      }
      await browser.keys("Escape");
      await expectPanelFocus();
      const hiddenHeight = await browser.execute(() => document.querySelector(".layout").getBoundingClientRect().height);
      if (Math.abs(hiddenHeight - shownHeight - barHeight) > 2) {
        throw new Error(`Hidden button bar did not reclaim its height: ${shownHeight}, ${hiddenHeight}, ${barHeight}`);
      }

      // File operations and zoom must still work when the bar renders nothing.
      await browser.keys(isMac ? "7" : "F7");
      await browser.$('[data-testid="mkdir-name-input"]').waitForDisplayed();
      await browser.keys("Escape");
      await expectPanelFocus();
      const fontSize = await browser.execute(() => getComputedStyle(document.documentElement).getPropertyValue("--font-size"));
      const zoomStart = observer.messages.length;
      await pressWindowKey(browser, "-", modifier);
      await waitFor(async () => observer.messages.slice(zoomStart).some((message) =>
        message.type === "StatePatch" && message.patch.fontSize === parseFloat(fontSize) - 1)
        && await browser.execute((previous) =>
          getComputedStyle(document.documentElement).getPropertyValue("--font-size") !== previous, fontSize),
      { label: "zoom still works with hidden button bar" });
      await pressWindowKey(browser, "+", modifier);
      await waitFor(() => browser.execute((previous) =>
        getComputedStyle(document.documentElement).getPropertyValue("--font-size") === previous, fontSize),
      { label: "restore zoom after hidden button bar check" });

      await reloadBrowserApp(browser, "hidden button bar");
      await waitVisible(false);
      await browser.execute((selector) => document.querySelector(selector)?.focus(), panel);
      await pressWindowKey(browser, "p", { ...modifier, shift: true });
      const search = await browser.$(".command-palette__input");
      await search.waitForDisplayed();
      await search.setValue("Toggle Button Bar");
      await browser.keys("Enter");
      await waitVisible(true);
      await expectPanelFocus();
    }

    for (const value of ["false", 0, null]) {
      const start = observer.messages.length;
      observer.sendIntent("SetGeneralOption", undefined, { key: "showButtonBar", value });
      await waitFor(() => observer.messages.slice(start).some((message) =>
        message.type === "Error" && message.code === "BAD_SETTING"),
      { label: "reject invalid button bar setting" });
      observer.sendIntent("RequestStateSnapshot");
      await waitFor(() => observer.messages.slice(start).some((message) => message.type === "StateSnapshot"),
        { label: "authoritative button bar state after rejection" });
      if (observer.state.settings.general.showButtonBar !== true) {
        throw new Error("Invalid button bar setting changed visibility");
      }
    }
  } finally {
    await browser.keys("Escape");
    observer.sendIntent("SetGeneralOption", undefined, { key: "showButtonBar", value: previous });
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    await waitVisible(previous);
    peer.close();
    await goToPanelPath(browser, observer, "left", previousPath);
    fs.rmSync(fixture, { recursive: true, force: true });
  }
}
