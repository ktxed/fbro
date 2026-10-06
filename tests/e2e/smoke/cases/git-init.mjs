import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickEntry, goToPanelPath, pressWindowKey, setTextInputValue } from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

const dialog = '[data-testid="git-init-dialog"]';
const input = '[data-testid="git-init-branch-input"]';
const confirm = '[data-command-id="git.init.confirm"]';
const cancel = '[data-command-id="git.init.cancel"]';
const panel = '[data-testid="panel-left"]';

export async function runGitInitFlow(browser, observer) {
  const hostDir = fs.mkdtempSync(hostPath(backendPath("a", "src", "git-init-")));
  const dir = backendPath("a", "src", path.basename(hostDir));
  const menuFolder = `${dir}/from-menu`;
  const paletteFolder = `${dir}/from-palette`;
  const emptyFolder = `${dir}/empty-repository`;
  fs.mkdirSync(hostPath(menuFolder));
  fs.mkdirSync(hostPath(paletteFolder));
  fs.mkdirSync(hostPath(emptyFolder));
  fs.writeFileSync(hostPath(`${menuFolder}/notes.txt`), "existing work\n");
  fs.writeFileSync(hostPath(`${paletteFolder}/notes.txt`), "palette work\n");
  await browser.execute(() => {
    window.__gitInitSmokeIntents = [];
    window.__gitInitSmokeSend = WebSocket.prototype.send;
    WebSocket.prototype.send = function (data) {
      const message = typeof data === "string" ? JSON.parse(data) : null;
      if (message?.type === "Intent" && message.name === "InitGitRepository") {
        window.__gitInitSmokeIntents.push(message);
      }
      return window.__gitInitSmokeSend.call(this, data);
    };
  });
  try {
    await goToPanelPath(browser, observer, "left", dir);
    await clickEntry(browser, "left", "from-menu");
    await openFromMenu(browser);
    await expectModalFocus(browser, input, "Git init focuses branch input from menu");
    if (await (await browser.$(input)).getValue() !== "main") throw new Error("Git init did not default to main");
    await expectTarget(browser, menuFolder);
    await browser.keys("Tab");
    await expectModalFocus(browser, confirm, "Git init Tab reaches Initialize");
    await browser.keys("Tab");
    await expectModalFocus(browser, cancel, "Git init Tab reaches Cancel");
    await browser.keys("Enter");
    await expectClosedAndPanelFocus(browser);
    if (fs.existsSync(hostPath(`${menuFolder}/.git`))) throw new Error("Enter on Cancel initialized a repository");

    await openFromMenu(browser);
    await browser.keys("Escape");
    await expectClosedAndPanelFocus(browser);
    await openFromMenu(browser);
    await pressWindowKey(browser, "F7");
    await pressWindowKey(browser, "p", { ctrl: true, shift: true });
    const layers = await browser.execute(() => [...document.querySelectorAll("[data-modal-id]")].map(layer => layer.dataset.modalId));
    if (layers.join("|") !== "git-init") throw new Error(`Git init did not suppress global modal shortcuts: ${layers}`);
    await setTextInputValue(browser, input, "topic/start");
    await submitTwiceInOneEventTurn(browser);
    await expectClosedAndPanelFocus(browser);
    if (await sentCount(browser) !== 1) throw new Error("Git init input submitted more than once");
    await expectRepository(menuFolder, "topic/start");
    if (fs.existsSync(hostPath(`${dir}/.git`))) throw new Error("Context menu initialized the parent folder");
    if (fs.readFileSync(hostPath(`${menuFolder}/notes.txt`), "utf8") !== "existing work\n") throw new Error("Git init changed existing files");

    // The palette initializes its current folder even when a file is at the cursor.
    await goToPanelPath(browser, observer, "left", paletteFolder);
    await clickEntry(browser, "left", "notes.txt");
    await browser.keys("c");
    await (await browser.$(".context-menu")).waitForDisplayed({ timeout: 10_000 });
    if (await (await browser.$('.context-menu [data-command-id="git.init"]')).isExisting()) throw new Error("Git init appeared for a file");
    await browser.keys("Escape");
    await openFromPalette(browser);
    await expectModalFocus(browser, input, "Git init focuses branch input from palette");
    await expectTarget(browser, paletteFolder);
    await setTextInputValue(browser, input, "bad name");
    await browser.keys("Enter");
    await waitFor(async () => (await browser.$(`${dialog} [role="alert"]`)).isDisplayed(), { timeout: 10_000, label: "Git init invalid-name rejection" });
    if (await (await browser.$(input)).getValue() !== "bad name") throw new Error("Git init rejection cleared the branch name");
    if (fs.existsSync(hostPath(`${paletteFolder}/.git`))) throw new Error("Invalid name created Git metadata");
    await setTextInputValue(browser, input, "main");
    // Close immediately after submission, before a backend response can arrive.
    await browser.execute(({ input, cancel }) => {
      document.querySelector(input).dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
      document.querySelector(cancel).click();
    }, { input, cancel });
    await expectClosedAndPanelFocus(browser);
    await expectRepository(paletteFolder, "main");
    await waitFor(() => browser.execute(() => document.body.textContent.includes("Initialized Git repository")), { timeout: 10_000, label: "background Git init completion feedback" });
    await waitFor(() => browser.execute(() => document.querySelector('[data-testid="panel-left"] .panel__git-status')?.textContent === "?"), { timeout: 10_000, label: "Git init refreshes untracked file decoration" });
    if (await sentCount(browser) !== 3) throw new Error("Unexpected number of Git initialization intents");

    await openFromPalette(browser);
    await setTextInputValue(browser, input, "replacement");
    await browser.keys("Enter");
    await waitFor(async () => (await browser.$(`${dialog} [role="alert"]`)).isDisplayed(), { timeout: 10_000, label: "Git init existing-repository rejection" });
    if (gitOutput(paletteFolder, ["branch", "--show-current"]).trim() !== "main") throw new Error("Git init changed an existing repository branch");
    await browser.keys("Escape");
    await expectClosedAndPanelFocus(browser);

    // Initializing an existing folder under the new repository is intentional.
    const nested = `${paletteFolder}/nested`;
    fs.mkdirSync(hostPath(nested));
    await observer.waitForEntry("left", "nested", "directory");
    await clickEntry(browser, "left", "nested");
    await openFromMenu(browser);
    await expectTarget(browser, nested);
    await browser.keys("Enter");
    await expectClosedAndPanelFocus(browser);
    await expectRepository(nested, "main");
    if (gitOutput(paletteFolder, ["branch", "--show-current"]).trim() !== "main") throw new Error("Nested init changed its parent repository");

    // A genuinely empty repository still supports creation and Quick Edit.
    await goToPanelPath(browser, observer, "left", emptyFolder);
    await openFromPalette(browser);
    await browser.keys("Enter");
    await (await browser.$(dialog)).waitForDisplayed({ timeout: 10_000, reverse: true });
    await expectRepository(emptyFolder, "main");
    await waitFor(() => browser.execute(() => Boolean(
      document.querySelector('[data-testid="panel-left"] [data-entry-name=".git"]')
        || document.querySelector('[data-testid="panel-left"] [data-testid="empty-folder-new-file"]'),
    )), { timeout: 10_000, label: "empty repository listing settles" });
    if (!await (await browser.$(`${panel} [data-entry-name=".git"]`)).isExisting()) {
      await browser.keys(".");
    }
    await (await browser.$(`${panel} [data-entry-name=".git"]`)).waitForDisplayed({ timeout: 10_000 });
    if (await (await browser.$(`${panel} [data-testid="panel-empty-folder"]`)).isExisting()) throw new Error("Visible .git directory was reported as an empty listing");
    await browser.keys(".");
    await waitFor(() => browser.execute(() => Boolean(document.querySelector('[data-testid="panel-left"] [data-testid="empty-folder-new-file"]'))), { timeout: 10_000, label: "empty repository creation suggestion" });
    await (await browser.$(`${panel} [data-testid="empty-folder-new-file"]`)).click();
    await expectModalFocus(browser, '[data-testid="touch-name-input"]', "empty repository new file name focus");
    await setTextInputValue(browser, '[data-testid="touch-name-input"]', "new.txt");
    await browser.keys("Enter");
    await observer.waitForEntry("left", "new.txt", "file");
    await waitFor(() => browser.execute(() => document.activeElement === document.querySelector('[data-testid="panel-left"]')), { timeout: 10_000, label: "empty repository new file returns listing focus" });
    await browser.performActions([{
      type: "key", id: "git-empty-quick-edit", actions: [
        { type: "keyDown", value: "\uE009" }, { type: "keyDown", value: "\uE034" },
        { type: "keyUp", value: "\uE034" }, { type: "keyUp", value: "\uE009" },
      ],
    }]);
    await browser.releaseActions();
    await waitFor(() => fs.readFileSync(hostPath(`${emptyFolder}/new.txt`), "utf8").includes("quick edit saved"), { timeout: 10_000, label: "Ctrl+F4 edits a newly created file in an empty Git repository" });
  } finally {
    await browser.execute(() => {
      WebSocket.prototype.send = window.__gitInitSmokeSend;
      delete window.__gitInitSmokeSend;
      delete window.__gitInitSmokeIntents;
    });
  }
}

async function openFromMenu(browser) {
  await browser.keys("c");
  await (await browser.$('.context-menu [data-command-id="git.init"]')).waitForDisplayed({ timeout: 10_000 });
  await (await browser.$('.context-menu [data-command-id="git.init"]')).click();
  await (await browser.$(dialog)).waitForDisplayed({ timeout: 10_000 });
}

async function openFromPalette(browser) {
  await pressWindowKey(browser, "p", { meta: true, shift: true });
  await pressWindowKey(browser, "p", { ctrl: true, shift: true });
  await (await browser.$(".command-palette__input")).waitForDisplayed({ timeout: 10_000 });
  await setTextInputValue(browser, ".command-palette__input", "Initialize Git repository");
  await browser.keys("Enter");
  await (await browser.$(dialog)).waitForDisplayed({ timeout: 10_000 });
}

async function expectTarget(browser, target) {
  if (await (await browser.$('[data-testid="git-init-target"]')).getAttribute("title") !== target) throw new Error(`Git init has the wrong target; expected ${target}`);
}

async function expectClosedAndPanelFocus(browser) {
  await (await browser.$(dialog)).waitForDisplayed({ timeout: 10_000, reverse: true });
  await waitFor(async () => (await browser.$(panel)).isFocused(), { timeout: 10_000, label: "Git init restores panel focus" });
}

async function submitTwiceInOneEventTurn(browser) {
  await browser.execute((input) => {
    const field = document.querySelector(input);
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  }, input);
}

async function sentCount(browser) {
  return browser.execute(() => window.__gitInitSmokeIntents.length);
}

function gitOutput(folder, args) {
  return execFileSync("git", ["-C", hostPath(folder), ...args], { encoding: "utf8" });
}

async function expectRepository(folder, branch) {
  await waitFor(() => fs.existsSync(hostPath(`${folder}/.git/HEAD`)), { timeout: 10_000, label: `Git initialized ${folder}` });
  if (gitOutput(folder, ["branch", "--show-current"]).trim() !== branch) throw new Error(`Git init did not use ${branch}`);
  if (gitOutput(folder, ["rev-list", "--all"]).trim()) throw new Error("Git init created a commit");
  if (gitOutput(folder, ["diff", "--cached", "--name-only"]).trim()) throw new Error("Git init staged files");
}
