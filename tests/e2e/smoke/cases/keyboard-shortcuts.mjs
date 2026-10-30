import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { artifactsDir } from "../config.mjs";
import { pressWindowKey, reloadBrowserApp } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runKeyboardShortcutsFlow(browser, observer) {
  const previousScheme = observer.state.settings.general.colorScheme;
  const previousFontSize = await browser.execute(() => parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")));
  const selector = id => `[data-testid="${id}"]`;
  const click = async id => { const button = await browser.$(selector(id)); await button.waitForClickable(); await button.click(); };
  const exists = async id => (await browser.$(selector(id))).isExisting();
  const key = async value => browser.keys(value);
  const focused = async id => browser.execute(id => document.activeElement === document.querySelector(`[data-testid="${id}"]`), id);
  const choose = async id => {
    const input = await browser.$(selector("keyboard-search"));
    await input.setValue(id);
    await browser.$(`.keyboard-settings [data-command-id="${id}"]`).waitForDisplayed();
  };
  const bindings = id => observer.state.commands[id].bindings;
  const waitClosed = async () => waitFor(async () => !await exists("keyboard-recorder"), { label: "shortcut recorder closed" });
  const open = async () => {
    await pressWindowKey(browser, ",", { ctrl: true });
    await browser.$(selector("settings-tab-general")).waitForDisplayed();
    await key("End");
    await browser.$(selector("keyboard-search")).waitForDisplayed();
  };
  const reset = async () => {
    const start = observer.messages.length;
    observer.sendIntent("ResetAllCommandBindings", undefined, { requestId: crypto.randomUUID(), revision: observer.state.commandRevision });
    await waitFor(() => observer.messages.slice(start).some(message => message.type === "CommandBindingsOk"), { label: "restore shortcut defaults" });
  };
  try {
    await reset();
    await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
    await open();
    const sidebar = await browser.execute(() => [...document.querySelectorAll('.settings-modal__sidebar [role="tab"]')].map(tab => ({
      label: tab.textContent.trim(), icon: tab.querySelector('svg')?.getAttribute('aria-hidden'),
    })));
    assert.deepEqual(sidebar.map(tab => tab.label), ["General", "Display", "Bros", "Sharing", "Shortcuts"]);
    assert.ok(sidebar.every(tab => tab.icon === "true"), "every settings tab has a decorative icon");
    await key("Home");
    for (const id of ["display", "bros", "sharing", "keyboard"]) {
      await key("ArrowDown");
      assert.equal(await focused(`settings-tab-${id}`), true, "settings sidebar keeps roving focus");
    }
    // First edit entirely by keyboard: tab from sidebar to search, filter, command, Add.
    await key("Tab");
    assert.equal(await focused("keyboard-search"), true);
    for (const character of "app.about") await browser.action("key").down(character).up(character).perform();
    assert.equal(await (await browser.$(selector("keyboard-search"))).getValue(), "app.about");
    await key("Tab"); await key("Tab"); await key("Tab");
    assert.equal(await focused("keyboard-add"), true);
    await key("Enter");
    await browser.$(selector("keyboard-recorder")).waitForDisplayed();
    await key("F12");
    assert.equal(await exists("keyboard-recorder"), true);
    await key("Tab"); await key("Tab"); await key("Tab");
    assert.equal(await focused("keyboard-save"), true);
    await key("Enter"); await waitClosed();
    await waitFor(() => bindings("app.about")[0]?.strokes[0].key === "F12", { label: "F12 saved" });
    await waitFor(() => focused("keyboard-add"), { label: "recorder restores Add opener" });
    await key("Escape");
    await pressWindowKey(browser, "p", { ctrl: true, shift: true });
    await browser.$(".command-palette__input").waitForDisplayed();
    await (await browser.$(".command-palette__input")).setValue("app.about");
    const aboutLayout = await browser.execute(() => {
      const icon = document.querySelector('img.command-palette__item-icon, .command-palette__item-icon img');
      const row = icon?.closest('button');
      const binding = row?.querySelector('.command-palette__item-binding');
      const main = row?.querySelector('.command-palette__item-main');
      if (!icon || !binding || !main) return null;
      const image = icon.getBoundingClientRect();
      const shortcut = binding.getBoundingClientRect();
      return { gap: main.getBoundingClientRect().left - image.right, loaded: icon.complete && icon.naturalWidth > 0, binding: binding.textContent, trailing: row.getBoundingClientRect().right - shortcut.right };
    });
    assert.ok(aboutLayout?.loaded, "About logo loads with a customized shortcut");
    assert.equal(aboutLayout.binding, "F12");
    assert.ok(aboutLayout.gap >= 0 && aboutLayout.gap <= 20, "About logo precedes its command title");
    assert.ok(aboutLayout.trailing >= 0 && aboutLayout.trailing <= 20, "customized About shortcut remains right-aligned");
    await browser.saveScreenshot(path.join(artifactsDir, "screenshots", "palette-about-custom-shortcut.png"));
    await (await browser.$(".command-palette__input")).click();
    await browser.keys(["Control", "a"]);
    await key("Backspace");
    await browser.$('.command-palette [data-command-id="app.search"]').waitForDisplayed();
    for (const scheme of ["fbro-dark", "daylight"]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      observer.sendIntent("SetFontSize", undefined, { value: 20 });
      await waitFor(() => browser.execute(scheme => document.querySelector(".app")?.dataset.colorScheme === scheme
        && parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")) === 20, scheme), { label: `palette icons ${scheme} large text` });
      const paletteIcons = await browser.execute(() => {
        const row = id => document.querySelector(`.command-palette [data-command-id="${id}"]`);
        return {
          actions: ["app.search", "app.settings", "file.copy", "panel.cursor.up", "panel.selection.extend_up", "panel.selection.calculate_sizes"].every(id => row(id)?.querySelector('.command-palette__item-icon svg')
            && row(id).querySelector('.command-palette__item-icon').getAttribute('aria-hidden') === 'true'),
          complete: [...document.querySelectorAll('.command-palette__item')].every(item => item.querySelector('.command-palette__item-icon svg, .command-palette__item-icon img')),
          titles: ["app.search", "app.about", "panel.cursor.up"].map(id => row(id)?.querySelector('.command-palette__item-main')?.getBoundingClientRect().left),
        };
      });
      assert.equal(paletteIcons.actions, true, "navigation, selection, and familiar actions have decorative icons");
      assert.equal(paletteIcons.complete, true, "every command has a visual cue");
      assert.ok(paletteIcons.titles.every(left => Number.isFinite(left) && Math.abs(left - paletteIcons.titles[0]) <= 1), "titles stay aligned across different command icons");
      await browser.execute(() => { document.querySelector('.command-palette__list').scrollTop = 0; });
      await browser.saveScreenshot(path.join(artifactsDir, "screenshots", `palette-leading-icons-${scheme}.png`));
      await browser.execute(() => document.querySelector('.command-palette [data-command-id="panel.cursor.up"]').scrollIntoView({ block: "start" }));
      await browser.saveScreenshot(path.join(artifactsDir, "screenshots", `palette-navigation-icons-${scheme}.png`));
    }
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    observer.sendIntent("SetFontSize", undefined, { value: previousFontSize });
    await waitFor(() => browser.execute(size => parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")) === size, previousFontSize), { label: "restore palette font size" });
    await key("Escape");
    await key("F12"); await browser.$('[data-modal-id="about"]').waitForDisplayed();
    await key("Escape");
    await open(); await choose("app.about");
    // Review the long command name, both themes, and large text in the native modal.
    await choose("app.command_palette");
    for (const scheme of ["fbro-dark", "daylight"]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      observer.sendIntent("SetFontSize", undefined, { value: 20 });
      await waitFor(() => browser.execute(scheme => document.querySelector(".app")?.dataset.colorScheme === scheme
        && parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")) === 20, scheme), { label: `shortcut editor ${scheme} large text` });
      const singleLineTabs = await browser.execute(() => [...document.querySelectorAll('.settings-modal__tab span')].every(label => {
        const range = document.createRange();
        range.selectNodeContents(label);
        return range.getClientRects().length === 1;
      }));
      assert.equal(singleLineTabs, true, "settings labels stay on one line with large text");
      await browser.saveScreenshot(path.join(artifactsDir, "screenshots", `settings-sidebar-${scheme}.png`));
      await click("keyboard-edit-0");
      await browser.$(selector("keyboard-recorder")).waitForDisplayed();
      const captureFocus = await browser.execute(() => {
        const capture = document.querySelector('[data-testid="keyboard-capture"]');
        return { focused: document.activeElement === capture, outline: getComputedStyle(capture).outlineStyle };
      });
      assert.deepEqual(captureFocus, { focused: true, outline: "none" }, "capture has one integrated focus border");
      await browser.saveScreenshot(path.join(artifactsDir, "screenshots", `keyboard-recorder-${scheme}.png`));
      await pressWindowKey(browser, "p", { ctrl: true, shift: true });
      const fits = await browser.execute(() => {
        const dialog = document.querySelector('[data-testid="keyboard-recorder"]');
        const bounds = dialog.getBoundingClientRect();
        return bounds.left >= 0 && bounds.right <= innerWidth && bounds.top >= 0 && bounds.bottom <= innerHeight
          && dialog.scrollWidth <= dialog.clientWidth
          && [...dialog.querySelectorAll('select, button')].every(control => {
            const rect = control.getBoundingClientRect();
            return rect.left >= bounds.left && rect.right <= bounds.right;
          });
      });
      const controlHeights = await browser.execute(() => ['keyboard-context', 'keyboard-record-again'].map(id =>
        document.querySelector(`[data-testid="${id}"]`).getBoundingClientRect().height));
      assert.ok(Math.abs(controlHeights[0] - controlHeights[1]) <= 1, "context and Record again have aligned control heights");
      assert.equal(fits, true, `${scheme} shortcut editor fits large text without clipping controls`);
      await browser.saveScreenshot(path.join(artifactsDir, "screenshots", `keyboard-shortcut-review-${scheme}.png`));
      await click("keyboard-record-again");
      assert.equal(await (await browser.$(selector("keyboard-capture"))).getAttribute("data-recording"), "true");
      await key("Escape");
      assert.equal(await exists("keyboard-recorder"), true);
      await key("Escape"); await waitClosed();
      await waitFor(() => focused("keyboard-edit-0"), { label: "recorder restores Edit opener" });
    }
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    observer.sendIntent("SetFontSize", undefined, { value: previousFontSize });
    await waitFor(() => browser.execute(size => parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")) === size, previousFontSize), { label: "restore editor font size" });
    await choose("app.about");
    // Both-view conflicts emphasize the affected command and keep Abort safe.
    for (const scheme of ["fbro-dark", "daylight"]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      observer.sendIntent("SetFontSize", undefined, { value: 20 });
      await waitFor(() => browser.execute(scheme => document.querySelector(".app")?.dataset.colorScheme === scheme
        && parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")) === 20, scheme), { label: `conflict warning ${scheme} large text` });
      await click("keyboard-add"); await key("s"); await click("keyboard-save");
      await browser.$(selector("keyboard-abort")).waitForDisplayed();
      assert.equal(await focused("keyboard-abort"), true);
      assert.equal(await (await browser.$(selector("keyboard-conflict-command"))).getText(), "Sync Other Panel To This Path");
      const viewLabels = await browser.execute(() => [...document.querySelectorAll('[data-testid="keyboard-conflict-context"]')].map(badge => badge.textContent));
      assert.deepEqual(viewLabels, ["List view", "Grid view"]);
      const contained = await browser.execute(() => {
        const dialog = document.querySelector('.shortcut-conflicts');
        return dialog.scrollWidth <= dialog.clientWidth && !dialog.textContent.includes('panel.path.sync');
      });
      assert.equal(contained, true, "command warning has readable names without clipping or registry IDs");
      await browser.saveScreenshot(path.join(artifactsDir, "screenshots", `shortcut-conflicts-${scheme}.png`));
      await key("Escape");
      await waitFor(() => focused("keyboard-save"), { label: "conflict warning restores Save opener" });
      await key("Escape"); await waitClosed();
    }
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    observer.sendIntent("SetFontSize", undefined, { value: previousFontSize });
    await waitFor(() => browser.execute(size => parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--font-size")) === size, previousFontSize), { label: "restore conflict warning font size" });
    // Native Tab, Enter, Escape all complete capture without navigation or activation.
    for (const captured of ["Tab", "Enter", "Escape"]) {
      await click("keyboard-add"); await key(captured);
      assert.equal(await exists("keyboard-recorder"), true);
      assert.match(await (await browser.$(selector("keyboard-capture"))).getText(), new RegExp(captured));
      await key("Escape"); await waitClosed();
    }
    // Zoom is suppressed while recording, and rejected as reserved after capture.
    const fontSize = await browser.execute(() => getComputedStyle(document.documentElement).getPropertyValue("--font-size"));
    await click("keyboard-add"); await pressWindowKey(browser, "-", {ctrl:true});
    assert.equal(await browser.execute(() => getComputedStyle(document.documentElement).getPropertyValue("--font-size")), fontSize);
    await click("keyboard-save");
    await browser.$(selector("keyboard-error")).waitForDisplayed();
    assert.match(await (await browser.$(selector("keyboard-error"))).getText(), /reserved/i);
    await key("Escape"); await waitClosed();
    // Add a conflicting List-only binding. Abort leaves the registry unchanged.
    await click("keyboard-add"); await key("F2");
    await (await browser.$(selector("keyboard-context"))).selectByAttribute("value", "list");
    const before = JSON.stringify(observer.state.commands);
    await click("keyboard-save"); await browser.$(selector("keyboard-abort")).waitForDisplayed();
    assert.equal(await focused("keyboard-abort"), true);
    await key("Enter");
    assert.equal(JSON.stringify(observer.state.commands), before);
    await click("keyboard-save"); await click("keyboard-override"); await waitClosed();
    await waitFor(() => bindings("file.rename")[0]?.modes[0] === "grid", { label: "partial override leaves Grid-only Rename" });
    assert.equal(bindings("app.about")[0].strokes[0].key, "F12");
    assert.equal(bindings("app.about")[1].strokes[0].key, "F2");
    // Duplicate bindings stay in the editor with an inline explanation.
    await click("keyboard-add"); await key("F12"); await click("keyboard-save");
    await browser.$(selector("keyboard-error")).waitForDisplayed();
    await key("Escape"); await waitClosed();
    // Editing preserves the primary position and updates toolbar hints immediately.
    await choose("file.rename"); await click("keyboard-edit-0"); await key("F11"); await click("keyboard-save"); await waitClosed();
    await waitFor(() => browser.execute(() => document.querySelector('.toolbar [data-command-id="file.rename"] .toolbar__key')?.textContent === "F11"), { label: "live toolbar label" });
    await click("keyboard-remove-all");
    await waitFor(() => bindings("file.rename").length === 0, { label: "explicit unassignment" });
    await waitFor(() => browser.execute(() => document.querySelector('.toolbar [data-command-id="file.rename"] .toolbar__key')?.textContent === ""), { label: "removed toolbar hint" });
    // Resetting Rename now conflicts with About's F2. Abort, then override.
    await click("keyboard-reset"); await click("keyboard-abort");
    assert.equal(bindings("file.rename").length, 0);
    await click("keyboard-reset"); await click("keyboard-override");
    await waitFor(() => !observer.state.commands["file.rename"].customized, { label: "per-command reset" });
    assert.equal(bindings("app.about").length, 1);
    await choose("app.about"); await click("keyboard-remove-0");
    await waitFor(() => bindings("app.about").length === 0, { label: "remove individual shortcut" });
    // Restore an alternate binding and reload the frontend against the persisted backend.
    await click("keyboard-add"); await key("F12"); await click("keyboard-save"); await waitClosed();
    await key("Escape"); await reloadBrowserApp(browser, "keyboard shortcuts");
    await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
    await key("F12"); await browser.$('[data-modal-id="about"]').waitForDisplayed(); await key("Escape");
    await open(); await choose("app.about");
    await click("keyboard-reset-all");
    assert.equal(await focused("keyboard-cancel-reset-all"), true);
    await key("Enter"); assert.equal(bindings("app.about").length, 1);
    await click("keyboard-reset-all"); await click("keyboard-confirm-reset-all");
    await waitFor(() => Object.values(observer.state.commands).every(command => !command.customized), { label: "reset all clears every override" });
    await waitFor(() => focused("keyboard-reset-all"), { label: "reset confirmation restores opener" });
    await browser.saveScreenshot(path.join(artifactsDir, "keyboard-shortcuts.png"));
    await key("Escape");
    await waitFor(() => browser.execute(() => document.activeElement?.matches('[data-testid="panel-left"]')), { label: "settings restores panel opener" });
    console.log("PASS keyboard shortcuts: capture, live bindings, ownership, conflicts, resets, reload, focus");
  } finally {
    fs.mkdirSync(artifactsDir, {recursive:true});
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: previousScheme });
    observer.sendIntent("SetFontSize", undefined, { value: previousFontSize });
    await reset();
  }
}
