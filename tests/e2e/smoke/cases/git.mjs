import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { goToPanelPath } from "../browser.mjs";
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
  fs.writeFileSync(path.join(hostRepo, "removed.txt"), "restore me\n");
  git(hostRepo, ["add", "changed.txt", "stage.txt", "removed.txt"]);
  git(hostRepo, ["commit", "-qm", "initial"]);
  fs.writeFileSync(path.join(hostRepo, "changed.txt"), "after\n");
  fs.writeFileSync(path.join(hostRepo, "stage.txt"), "after stage\n");
  fs.rmSync(path.join(hostRepo, "removed.txt"));
  fs.writeFileSync(path.join(hostRepo, "untracked.txt"), "remove me\n");
  git(hostRepo, ["add", "removed.txt"]);

  await goToPanelPath(browser, observer, "left", repo);
  await observer.waitForEntry("left", "changed.txt", "file");
  const gitButton = await browser.$('[data-testid="panel-left"] .panel__git-btn');
  await gitButton.waitForClickable({ timeout: 10_000 });
  await gitButton.click();
  const gitModal = await browser.$(".git-modal");
  await gitModal.waitForDisplayed({ timeout: 10_000 });
  await waitFor(() => gitChangeCode(browser, "changed.txt"), {
    timeout: 10_000,
    label: "Git change row",
  });
  await waitFor(async () => (await gitChangeCode(browser, "removed.txt")) === "D ", {
    timeout: 10_000,
    label: "Staged Git deletion",
  });
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

  await browser.keys(" ");
  await waitFor(async () => (await gitSelectionCount(browser)) === "1 selected", {
    timeout: 10_000,
    label: "Git keyboard selection",
  });
  await browser.keys(" ");
  await waitFor(async () => (await gitSelectionCount(browser)) === "", {
    timeout: 10_000,
    label: "Git keyboard deselection",
  });

  const changeCount = await browser.execute(() => document.querySelectorAll('[data-testid="git-change-row"]').length);
  await (await browser.$('[data-testid="git-select-all-toggle"]')).click();
  await waitFor(async () => (await gitSelectionCount(browser)) === `${changeCount} selected`, {
    timeout: 10_000,
    label: "Git select all",
  });
  await (await browser.$('[data-testid="git-select-all-toggle"]')).click();
  await waitFor(async () => (await gitSelectionCount(browser)) === "", {
    timeout: 10_000,
    label: "Git clear all",
  });

  await browser.execute(() => {
    const row = [...document.querySelectorAll('[data-testid="git-change-row"]')]
      .find((candidate) => candidate.querySelector('span[title="stage.txt"]'));
    row?.focus();
  });
  await browser.keys("s");
  await waitFor(async () => (await gitChangeCode(browser, "stage.txt")) === "M ", {
    timeout: 10_000,
    label: "Git keyboard stage",
  });
  await browser.keys("s");
  await waitFor(async () => (await gitChangeCode(browser, "stage.txt")) === " M", {
    timeout: 10_000,
    label: "Git keyboard unstage",
  });

  const rangePaths = await browser.execute(() => [...document.querySelectorAll('[data-testid="git-change-row"] span[title]')]
    .slice(0, 2)
    .map((element) => element.getAttribute("title")));
  if (rangePaths.length !== 2 || rangePaths.some((path) => !path)) {
    throw new Error("Git range selection requires two visible change rows");
  }
  await selectGitChange(browser, rangePaths[0]);
  await waitFor(async () => (await gitSelectionCount(browser)) === "1 selected", {
    timeout: 10_000,
    label: "Git range anchor",
  });
  await selectGitChange(browser, rangePaths[1], true);
  await waitFor(async () => (await gitSelectionCount(browser)) === "2 selected", {
    timeout: 10_000,
    label: "Git range selection",
  });
  await (await browser.$('[data-testid="git-clear-selection"]')).click();
  await selectGitChange(browser, "changed.txt");
  await selectGitChange(browser, "stage.txt");
  await waitFor(async () => (await gitSelectionCount(browser)) === "2 selected", {
    timeout: 10_000,
    label: "Git batch selection",
  });
  await (await browser.$('[data-testid="git-stage-selected"]')).click();
  await waitFor(async () => (await Promise.all(["changed.txt", "stage.txt"].map(gitChangeCode.bind(null, browser)))).every((code) => code === "M "), {
    timeout: 10_000,
    label: "Git batch stage",
  });
  await (await browser.$('[data-testid="git-unstage-selected"]')).click();
  await waitFor(async () => (await Promise.all(["changed.txt", "stage.txt"].map(gitChangeCode.bind(null, browser)))).every((code) => code === " M"), {
    timeout: 10_000,
    label: "Git batch unstage",
  });
  await (await browser.$('[data-testid="git-clear-selection"]')).click();

  await selectGitChange(browser, "removed.txt");
  await selectGitChange(browser, "untracked.txt");
  await (await browser.$('[data-testid="git-rollback-selected"]')).click();
  await waitFor(async () => (await browser.execute(() => document.querySelector('[data-testid="git-rollback-selected"]')?.textContent?.trim())) === "Confirm rollback 2", {
    timeout: 10_000,
    label: "Git rollback confirmation",
  });
  await (await browser.$('[data-testid="git-rollback-selected"]')).click();
  await waitFor(async () => !(await gitChangeCode(browser, "removed.txt")) && !(await gitChangeCode(browser, "untracked.txt")), {
    timeout: 10_000,
    label: "Git batch rollback",
  });
  if (!fs.existsSync(path.join(hostRepo, "removed.txt")) || fs.existsSync(path.join(hostRepo, "untracked.txt"))) {
    throw new Error("Git rollback did not restore the staged deletion and remove the untracked file");
  }

  await browser.keys("Escape");
  await gitModal.waitForExist({ reverse: true, timeout: 10_000 });

  observer.sendIntent("GitStagePaths", undefined, { panelId: "left", paths: [] });
  observer.sendIntent("GitViewDiff", undefined, { panelId: "left" });
  observer.sendIntent("NavigateToSource", "left", { sourceId: "journal" });
  await waitFor(() => {
    const panel = observer.state.panels.left;
    if (panel?.sourceId !== "journal" || panel.status !== "ready") return false;
    const names = panel.entries.map((entry) => entry.name);
    const expectedPrefixes = [
      "git-stage_v1_intent_",
      "git-stage_v1_completed_",
      "git-stage_v1_failed_",
      "git-unstage_v1_intent_",
      "git-unstage_v1_completed_",
      "git-rollback_v1_intent_",
      "git-rollback_v1_completed_",
      "git-view-diff_v1_intent_",
      "git-view-diff_v1_failed_",
    ];
    return expectedPrefixes.every((prefix) => names.some((name) => name.startsWith(prefix)));
  }, { timeout: 10_000, label: "semantic Git journal events" });

  const localSourceId = observer.sourceIdForPath(repo);
  if (!localSourceId) throw new Error(`No local source contains ${repo}`);
  observer.sendIntent("NavigateToSource", "left", { sourceId: localSourceId });
  await waitFor(() => observer.state.panels.left?.sourceId === localSourceId, {
    timeout: 10_000,
    label: "restore local source after Git journal assertions",
  });
  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: repo });
  await observer.waitForPanelPath("left", repo);
  await observer.waitForPanelReady("left");
}

async function selectGitChange(browser, path, shift = false) {
  const selector = `[data-testid="git-change-select"][aria-label="Select ${path}"]`;
  const checkbox = await browser.$(selector);
  await checkbox.waitForClickable({ timeout: 10_000 });
  if (shift) {
    await browser.execute((selector) => document.querySelector(selector)?.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true })), selector);
  } else {
    await checkbox.click();
  }
}

async function gitChangeCode(browser, path) {
  return browser.execute((path) => {
    const row = [...document.querySelectorAll('[data-testid="git-change-row"]')]
      .find((candidate) => candidate.querySelector('span[title]')?.getAttribute("title") === path);
    return row?.querySelector("code")?.textContent ?? null;
  }, path);
}

async function gitSelectionCount(browser) {
  return browser.execute(() => document.querySelector('[data-testid="git-selection-count"]')?.textContent?.trim() ?? "");
}

function git(cwd, args) {
  execFileSync("git", ["-C", cwd, ...args], { stdio: "pipe" });
}
