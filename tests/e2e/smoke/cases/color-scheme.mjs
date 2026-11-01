import { FbroObserver } from "../observer.mjs";
import { pressWindowKey, reloadBrowserApp, expectCheckboxSelected } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

// Settings → Display → "Match system appearance". The backend resolves the
// active scheme from the OS preference; in the Docker fixture there is no
// session bus, so `systemColorScheme` is null and the manual scheme applies.
export async function runColorSchemeFlow(browser, observer) {
  const general = (client = observer) => client.state.settings.general;
  const previousScheme = general().colorScheme;
  const previousFollow = general().followSystemColorScheme;
  const peer = new FbroObserver("color-scheme-peer", observer.url);
  const isMac = await browser.execute(() => navigator.platform.startsWith("Mac"));
  const modifier = isMac ? { meta: true } : { ctrl: true };
  const panel = '[data-testid="panel-left"]';
  const checkbox = '[data-testid="follow-system-color-scheme"]';
  const select = '[data-testid="color-scheme-select"]';
  const hint = '[data-testid="color-scheme-hint"]';

  const isFocused = (selector) => browser.execute((selector) =>
    document.activeElement === document.querySelector(selector), selector);
  const expectFocus = (selector, label) => waitFor(() => isFocused(selector), { timeout: 5000, label });
  const expectAppScheme = (scheme) => waitFor(() => browser.execute((scheme) =>
    document.querySelector(".app")?.dataset.colorScheme === scheme, scheme),
  { timeout: 10_000, label: `app renders ${scheme}` });
  const waitFollow = (follow) => waitFor(() =>
    general().followSystemColorScheme === follow && general(peer).followSystemColorScheme === follow,
  { timeout: 10_000, label: `follow system ${follow} in both clients` });
  const hintText = () => browser.execute((selector) => document.querySelector(selector)?.textContent ?? "", hint);
  const selectDisabled = () => browser.execute((selector) => document.querySelector(selector)?.disabled ?? null, select);
  const openDisplayFollowCheckbox = async () => {
    await browser.execute((selector) => document.querySelector(selector)?.focus(), panel);
    await pressWindowKey(browser, ",", modifier);
    await browser.$('[data-testid="settings-tab-general"]').waitForDisplayed();
    await browser.keys("ArrowDown");
    await browser.$(checkbox).waitForDisplayed();
    // Display controls: button bar checkbox, then the follow-system checkbox.
    await browser.keys("Tab");
    await browser.keys("Tab");
    await expectFocus(checkbox, "Display Tab reaches the follow-system checkbox");
  };
  const expectRejected = async (key, value) => {
    const start = observer.messages.length;
    observer.sendIntent("SetGeneralOption", undefined, { key, value });
    await waitFor(() => observer.messages.slice(start).some((message) =>
      message.type === "Error" && message.code === "BAD_SETTING"),
    { label: `reject ${key}=${JSON.stringify(value)}` });
  };

  try {
    await peer.connect();
    observer.sendIntent("SetGeneralOption", undefined, { key: "followSystemColorScheme", value: false });
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: "daylight" });
    await waitFor(() => general().colorScheme === "daylight" && general().activeColorScheme === "daylight"
      && general().followSystemColorScheme === false,
    { timeout: 10_000, label: "manual daylight scheme" });
    await expectAppScheme("daylight");

    await openDisplayFollowCheckbox();
    await expectCheckboxSelected(browser, checkbox, false, "follow-system checkbox");
    if (await selectDisabled() !== false) throw new Error("Manual scheme select disabled while not following");
    await browser.keys(" ");
    await waitFollow(true);
    await expectCheckboxSelected(browser, checkbox, true, "follow-system checkbox");
    await expectFocus(checkbox, "follow-system checkbox keeps focus after backend confirmation");

    const system = general().systemColorScheme;
    if (system === null || system === undefined) {
      await expectAppScheme("daylight");
      if (general().activeColorScheme !== "daylight") throw new Error("Unavailable system appearance changed the active scheme");
      if (await selectDisabled() !== false) throw new Error("Scheme select disabled while system appearance is unavailable");
      const text = await hintText();
      if (text !== "System appearance unavailable; using fbro Light.") throw new Error(`Unexpected hint: ${text}`);
    } else {
      const expected = system === "dark" ? "fbro-dark" : "daylight";
      await waitFor(() => general().activeColorScheme === expected, { label: `active scheme follows ${system}` });
      await expectAppScheme(expected);
      if (await selectDisabled() !== true) throw new Error("Scheme select stays enabled while following the system");
      const text = await hintText();
      if (!text.startsWith("Following system: ")) throw new Error(`Unexpected hint: ${text}`);
      await browser.keys("Tab");
      if (await isFocused(select)) throw new Error("Disabled scheme select is still a Tab stop");
      await browser.keys(["Shift", "Tab"]);
      await expectFocus(checkbox, "Shift+Tab returns to the follow-system checkbox");
    }

    // Read-only projections and malformed values are rejected.
    await expectRejected("activeColorScheme", "fbro-dark");
    await expectRejected("systemColorScheme", "dark");
    await expectRejected("followSystemColorScheme", "yes");
    if (general().followSystemColorScheme !== true) throw new Error("Rejected setting changed follow-system state");

    await browser.keys("Escape");
    await expectFocus(panel, "closing Settings restores panel focus");

    // The preference comes back from the backend snapshot after a reload.
    await reloadBrowserApp(browser, "follow system color scheme");
    await expectAppScheme(general().activeColorScheme);
    await openDisplayFollowCheckbox();
    await expectCheckboxSelected(browser, checkbox, true, "follow-system checkbox after reload");

    // Turning it off restores the preserved manual choice.
    await browser.keys(" ");
    await waitFollow(false);
    await waitFor(() => general().activeColorScheme === "daylight", { label: "manual scheme restored" });
    await expectAppScheme("daylight");
    if (await selectDisabled() !== false) throw new Error("Scheme select stays disabled after unfollowing");
    const value = await browser.execute((selector) => document.querySelector(selector)?.value, select);
    if (value !== "daylight") throw new Error(`Scheme select lost the manual choice: ${value}`);
    await browser.keys("Escape");
    await expectFocus(panel, "closing Settings restores panel focus after unfollowing");
  } finally {
    if (await browser.$('[data-testid="settings-tab-general"]').isExisting()) await browser.keys("Escape");
    observer.sendIntent("SetGeneralOption", undefined, { key: "followSystemColorScheme", value: previousFollow ?? false });
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    await waitFor(() => general().followSystemColorScheme === (previousFollow ?? false)
      && general().colorScheme === previousScheme,
    { timeout: 10_000, label: "restore color scheme settings" });
    peer.close();
  }
}
