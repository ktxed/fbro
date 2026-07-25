import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickContextMenuItem, clickEntry, goToPanelPath, openPanelContextMenu } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runGitFlow(browser, observer) {
  const repo = backendPath("a", "src", "git-diff-smoke");
  const hostRepo = hostPath(repo);
  fs.rmSync(hostRepo, { recursive: true, force: true });
  fs.mkdirSync(hostRepo, { recursive: true });
  git(hostRepo, ["init", "-q"]);
  git(hostRepo, ["config", "user.email", "fbro-tests@example.invalid"]);
  git(hostRepo, ["config", "user.name", "fbro tests"]);
  fs.writeFileSync(path.join(hostRepo, "changed.txt"), "before\n");
  fs.writeFileSync(path.join(hostRepo, "stage.txt"), "before stage\n");
  git(hostRepo, ["add", "changed.txt"]);
  git(hostRepo, ["add", "stage.txt"]);
  git(hostRepo, ["commit", "-qm", "initial"]);
  fs.writeFileSync(path.join(hostRepo, "changed.txt"), "after\n");
  fs.writeFileSync(path.join(hostRepo, "stage.txt"), "after stage\n");

  await goToPanelPath(browser, observer, "left", repo);
  await observer.waitForEntry("left", "changed.txt", "file");
  await waitFor(() => observer.state.panels.left?.git?.changes?.some((change) => change.path === "changed.txt") || false, {
    timeout: 10_000,
    label: "Git change status",
  });

  await clickEntry(browser, "left", "changed.txt");
  await openPanelContextMenu(browser, "left");
  await clickContextMenuItem(browser, "View Git diff");
  await assertAndCloseDiffWindow(browser, "before", "after");

  const gitButton = await browser.$('[data-testid="panel-left"] .panel__git-btn');
  await gitButton.waitForClickable({ timeout: 10_000 });
  await gitButton.click();
  const panelCursor = observer.state.panels.left?.cursor;
  await browser.keys("ArrowDown");
  await waitFor(async () => (await browser.$('[data-testid="git-change-row"][data-change-index="1"]')).isFocused(), {
    timeout: 10_000,
    label: "Git change row down navigation",
  });
  if (observer.state.panels.left?.cursor !== panelCursor) {
    throw new Error("Git modal arrow navigation changed the panel cursor");
  }
  await browser.keys("k");
  await waitFor(async () => (await browser.$('[data-testid="git-change-row"][data-change-index="0"]')).isFocused(), {
    timeout: 10_000,
    label: "Git change row K navigation",
  });
  await browser.keys("v");
  await assertAndCloseDiffWindow(browser, "before", "after");

  await browser.keys("j");
  await browser.keys("s");
  await waitFor(() => observer.state.panels.left?.git?.changes?.some((change) => change.path === "stage.txt" && change.indexStatus === "M") || false, {
    timeout: 10_000,
    label: "Git keyboard stage",
  });
  await browser.keys("s");
  await waitFor(() => observer.state.panels.left?.git?.changes?.some((change) => change.path === "stage.txt" && change.indexStatus === " ") || false, {
    timeout: 10_000,
    label: "Git keyboard unstage",
  });

  const modalDiffButton = await browser.$('[data-testid="git-view-diff"]');
  await modalDiffButton.waitForClickable({ timeout: 10_000 });
  await modalDiffButton.click();
  await assertAndCloseDiffWindow(browser, "before", "after");
}

async function assertAndCloseDiffWindow(browser, deleted, inserted) {
  const mainWindow = await browser.getWindowHandle();
  const diffWindow = await waitFor(async () => {
    const handles = await browser.getWindowHandles();
    return handles.find((handle) => handle !== mainWindow) || false;
  }, { timeout: 10_000, label: "Git diff window" });
  await browser.switchToWindow(diffWindow);
  const viewer = await browser.$('[data-testid="diff-viewer"]');
  await viewer.waitForDisplayed({ timeout: 10_000 });
  await waitFor(async () => {
    const rows = await browser.$$('[data-testid="diff-viewer"] [data-diff-row]');
    const text = await Promise.all(rows.map((row) => row.getText()));
    return text.some((row) => row.includes(deleted)) && text.some((row) => row.includes(inserted));
  }, { timeout: 10_000, label: "Git diff rows" });
  await browser.$('.diff-viewer__close').click();
  await waitFor(async () => !(await browser.getWindowHandles()).includes(diffWindow), {
    timeout: 10_000,
    label: "Git diff window close",
  });
  await browser.switchToWindow(mainWindow);
}

function git(cwd, args) {
  execFileSync("git", ["-C", cwd, ...args], { stdio: "pipe" });
}
