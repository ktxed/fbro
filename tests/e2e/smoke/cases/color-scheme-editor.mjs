import { pressWindowKey, setTextInputValue } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

// Settings → Display → Customize… → Color Scheme editor. The tab exists only in
// a Settings session opened from Display; Duplicate writes a TOML file through
// the backend; edits save (debounced) and restyle the whole app; Escape unwinds
// the open prompt before it closes Settings; and deleting the active scheme
// falls back to its base.
export async function runColorSchemeEditorFlow(browser, observer) {
  const general = () => observer.state.settings.general;
  const previousScheme = general().colorScheme;
  const previousFollow = general().followSystemColorScheme;
  const isMac = await browser.execute(() => navigator.platform.startsWith("Mac"));
  const modifier = isMac ? { meta: true } : { ctrl: true };
  const panel = '[data-testid="panel-left"]';
  const tabSelector = '[data-testid="settings-tab-colorScheme"]';
  const exists = (selector) => browser.$(selector).isExisting();
  const isFocused = (selector) => browser.execute((selector) =>
    document.activeElement === document.querySelector(selector), selector);
  const expectFocus = (selector, label) => waitFor(() => isFocused(selector), { timeout: 5000, label });
  const appVar = (name) => browser.execute((name) =>
    getComputedStyle(document.querySelector(".app")).getPropertyValue(name).trim(), name);
  const customItem = (name) => general().colorSchemes?.items.find((item) => item.name === name);
  const openSettings = async () => {
    await browser.execute((selector) => document.querySelector(selector)?.focus(), panel);
    await pressWindowKey(browser, ",", modifier);
    await browser.$('[data-testid="settings-tab-general"]').waitForDisplayed();
  };
  const closeSettings = async () => {
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="settings-tab-general"]')), { label: "settings closed" });
  };

  const name = `E2E Scheme ${Date.now()}`;
  let createdId;
  try {
    observer.sendIntent("SetGeneralOption", undefined, { key: "followSystemColorScheme", value: false });
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: "fbro-dark" });
    await waitFor(() => general().activeColorScheme === "fbro-dark", { label: "dark scheme active" });

    // Schemes shipped with fbro are listed, selectable, and read-only.
    const bundled = general().colorSchemes?.items.find((item) => item.id === "bundled:matrix");
    if (!bundled?.bundled) throw new Error("Bundled Matrix scheme missing from the catalog");
    const start = observer.messages.length;
    observer.sendIntent("UpdateColorScheme", undefined, { requestId: "bundled-edit", id: "bundled:matrix", colors: { bg: "#123456" }, terminal: {} });
    await waitFor(() => observer.messages.slice(start).some((m) =>
      m.type === "ColorSchemeFailed" && m.requestId === "bundled-edit" && m.code === "BAD_COLOR_SCHEME"),
    { label: "bundled scheme rejects edits" });

    // Not offered when Settings is opened normally.
    await openSettings();
    if (await exists(tabSelector)) throw new Error("Color Scheme tab visible without Customize…");

    // Display → Customize… reveals it directly.
    await browser.$('[data-testid="settings-tab-display"]').click();
    const customize = await browser.$('[data-testid="customize-color-scheme"]');
    await customize.waitForClickable();
    await customize.click();
    await browser.$('[data-testid="color-scheme-editor"]').waitForDisplayed();
    if (!(await exists(tabSelector))) throw new Error("Color Scheme tab missing after Customize…");

    // Built-ins are read-only; Duplicate is the way in.
    await browser.$('[data-testid="color-editor-readonly-hint"]').waitForDisplayed();
    const nameInputs = await browser.$$('[data-testid="color-editor-name"]');
    if (nameInputs.length !== 0) throw new Error("Built-in scheme exposes an editable name");

    await browser.$('[data-testid="color-editor-duplicate"]').click();
    await browser.$('[data-testid="color-editor-duplicate-name"]').waitForDisplayed();
    await expectFocus('[data-testid="color-editor-duplicate-name"]', "Duplicate prompt takes focus");

    // Escape closes the prompt first and restores focus to its opener.
    await browser.keys("Escape");
    await waitFor(async () => !(await exists('[data-testid="color-editor-duplicate-name"]')), { label: "prompt closed" });
    await expectFocus('[data-testid="color-editor-duplicate"]', "Escape returns focus to Duplicate");
    if (!(await exists('[data-testid="color-scheme-editor"]'))) throw new Error("Escape closed Settings instead of the prompt");

    await browser.$('[data-testid="color-editor-duplicate"]').click();
    await setTextInputValue(browser, '[data-testid="color-editor-duplicate-name"]', name);
    await browser.keys("Enter");
    await waitFor(() => customItem(name), { timeout: 10_000, label: "custom scheme created" });
    createdId = customItem(name).id;
    if (!/^custom:[a-z0-9_-]+$/.test(createdId)) throw new Error(`Unexpected scheme id ${createdId}`);
    if (!customItem(name).colors["bg-panel"]) throw new Error("Duplicate did not copy the template colors");
    await waitFor(() => browser.$('[data-testid="color-editor-name"]').isExisting(), { label: "new scheme selected and editable" });

    // The scheme list marks each scheme's kind: shipped ones are built-in.
    const optionText = (id) => browser.execute((id) =>
      document.querySelector(`[data-testid="color-editor-scheme-select"] option[value="${id}"]`)?.textContent ?? null, id);
    for (const [id, expected] of [["fbro-dark", "fbro Dark (built-in)"], ["bundled:matrix", "Matrix (built-in)"], [createdId, `${name} (custom)`]]) {
      const text = await optionText(id);
      if (text !== expected) throw new Error(`Scheme option ${id} reads ${JSON.stringify(text)}, expected ${JSON.stringify(expected)}`);
    }

    // Use it, then edit a color: the whole app restyles.
    await browser.$('[data-testid="color-editor-use"]').click();
    await waitFor(() => general().activeColorScheme === createdId, { timeout: 10_000, label: "custom scheme active" });
    await waitFor(() => browser.execute((id) => document.querySelector(".app")?.dataset.colorScheme === id, createdId),
      { label: "app renders the custom scheme" });

    await browser.$('[data-token-row="bg"]').click();
    await setTextInputValue(browser, '[data-testid="color-editor-value"]', "#123456");
    await waitFor(() => customItem(name)?.colors.bg === "#123456", { timeout: 10_000, label: "debounced save" });
    await waitFor(async () => (await appVar("--bg")) === "#123456", { label: "app uses the edited --bg" });

    // Invalid input is refused and never sent.
    await setTextInputValue(browser, '[data-testid="color-editor-value"]', "red;}");
    if (customItem(name).colors.bg !== "#123456") throw new Error("Invalid color reached the backend");

    // Clicking a preview element selects its token.
    await browser.$('[data-testid="color-scheme-preview"] [data-tokens~="accent"]').click();
    await waitFor(() => browser.execute(() =>
      document.querySelector('[data-token-row="accent"]')?.getAttribute("aria-selected") === "true"),
    { label: "preview click selects its token" });

    // Escape from the editor pane goes to the list first, then closes Settings.
    await browser.$('[data-testid="color-editor-value"]').click();
    await browser.keys("Escape");
    await expectFocus('[data-token-row="accent"]', "Escape returns focus to the token list");
    if (!(await exists('[data-testid="color-scheme-editor"]'))) throw new Error("Escape closed Settings from the editor pane");

    // Delete the active scheme: confirm inline, fall back to its base.
    await browser.$('[data-testid="color-editor-delete"]').click();
    await browser.$('[data-testid="color-editor-delete-confirm"]').waitForDisplayed();
    await browser.$('[data-testid="color-editor-delete-confirm"]').click();
    await waitFor(() => !customItem(name), { timeout: 10_000, label: "custom scheme deleted" });
    await waitFor(() => general().colorScheme === "fbro-dark" && general().activeColorScheme === "fbro-dark",
      { timeout: 10_000, label: "fell back to the base scheme" });
    createdId = undefined;

    // Closing Settings drops the tab; a normal reopen does not show it.
    await closeSettings();
    await expectFocus(panel, "closing Settings restores panel focus");
    await openSettings();
    if (await exists(tabSelector)) throw new Error("Color Scheme tab survived closing Settings");
    await closeSettings();
  } finally {
    if (await exists('[data-testid="settings-tab-general"]')) await browser.keys("Escape");
    if (createdId) observer.sendIntent("DeleteColorScheme", undefined, { requestId: crypto.randomUUID(), id: createdId });
    observer.sendIntent("SetGeneralOption", undefined, { key: "followSystemColorScheme", value: previousFollow ?? false });
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    await waitFor(() => general().followSystemColorScheme === (previousFollow ?? false)
      && general().colorScheme === previousScheme, { timeout: 10_000, label: "restore color scheme settings" });
  }
}
