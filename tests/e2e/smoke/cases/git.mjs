import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickEntry, goToPanelPath } from "../browser.mjs";
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
  fs.writeFileSync(path.join(hostRepo, "inline.txt"), "before inline\n");
  fs.mkdirSync(path.join(hostRepo, "nested"));
  fs.writeFileSync(path.join(hostRepo, "nested", "inside.txt"), "before nested\n");
  fs.writeFileSync(path.join(hostRepo, "stage.txt"), "before stage\n");
  fs.writeFileSync(path.join(hostRepo, "removed.txt"), "restore me\n");
  git(hostRepo, ["add", "changed.txt", "inline.txt", "nested/inside.txt", "stage.txt", "removed.txt"]);
  git(hostRepo, ["commit", "-qm", "initial"]);
  fs.writeFileSync(path.join(hostRepo, "changed.txt"), "after\n");
  fs.writeFileSync(path.join(hostRepo, "inline.txt"), "after inline\n");
  fs.writeFileSync(path.join(hostRepo, "nested", "inside.txt"), "after nested\n");
  fs.writeFileSync(path.join(hostRepo, "stage.txt"), "after stage\n");
  fs.rmSync(path.join(hostRepo, "removed.txt"));
  fs.writeFileSync(path.join(hostRepo, "untracked.txt"), "remove me\n");
  git(hostRepo, ["add", "removed.txt"]);

  await goToPanelPath(browser, observer, "left", repo);
  await observer.waitForEntry("left", "changed.txt", "file");

  await clickEntry(browser, "left", "inline.txt");
  const inlineDiff = await browser.$('[data-entry-name="inline.txt"] [data-testid="git-inline-view-diff"]');
  await inlineDiff.waitForDisplayed({ timeout: 10_000 });
  const inlineStage = await browser.$('[data-testid="git-inline-stage"]');
  await inlineStage.waitForDisplayed({ timeout: 10_000 });
  let inlineRollback = await browser.$('[data-testid="git-inline-rollback"]');
  await inlineRollback.waitForDisplayed({ timeout: 10_000 });
  const inlineActions = await browser.execute(() => [...document.querySelectorAll('[data-entry-name="inline.txt"] [data-inline-operation]')]
    .map(button => ({ id: button.getAttribute("data-inline-operation"), label: button.getAttribute("data-tooltip-content") })));
  const inlineActionIds = inlineActions.map(action => action.id).join("|");
  const inlineActionLabels = inlineActions.map(action => action.label).join("|");
  if (inlineActionIds !== "git.view-diff|git.stage|git.rollback|panel.pass-to-bro|file.copy|file.move|file.rename|file.delete"
    || inlineActionLabels !== "View Git diff|Stage|Rollback|Pass to Bro|Copy|Move|Rename|Delete") {
    throw new Error(`Unexpected inline Git actions: ${JSON.stringify(inlineActions)}`);
  }
  await inlineStage.moveTo();
  const iconTooltip = await browser.$(".panel__git-icon-tooltip");
  await iconTooltip.waitForDisplayed({ timeout: 10_000 });
  if ((await iconTooltip.getText()).trim() !== "Stage") throw new Error("Inline Stage tooltip has unexpected text");

  await browser.keys("ArrowRight");
  await waitFor(async () => (await browser.$('[data-testid="git-inline-view-diff"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git keyboard entry",
  });
  const cursorEntry = await browser.execute(() => document.querySelector('[data-testid="panel-left"] .panel__row--cursor')?.getAttribute("data-entry-name"));
  if (cursorEntry !== "inline.txt") throw new Error(`Inline Git keyboard entry changed the cursor to ${cursorEntry}`);

  await browser.keys("ArrowRight");
  await waitFor(async () => (await browser.$('[data-testid="git-inline-stage"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git keyboard right navigation",
  });
  await browser.keys("ArrowLeft");
  await waitFor(async () => (await browser.$('[data-testid="git-inline-view-diff"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git keyboard left navigation",
  });
  await browser.keys("ArrowLeft");
  await waitFor(async () => (await browser.$('[data-testid="panel-left"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git keyboard left exit",
  });
  await browser.keys("ArrowLeft");
  await observer.waitForPanelPath("left", path.dirname(repo));
  await goToPanelPath(browser, observer, "left", repo);
  await clickEntry(browser, "left", "inline.txt");
  const reEntryDiff = await browser.$('[data-entry-name="inline.txt"] [data-testid="git-inline-view-diff"]');
  await reEntryDiff.waitForDisplayed({ timeout: 10_000 });
  await browser.keys("ArrowRight");
  await waitFor(async () => (await browser.$('[data-testid="git-inline-view-diff"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git keyboard re-entry",
  });
  await browser.keys("Escape");
  await waitFor(async () => (await browser.$('[data-testid="panel-left"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git keyboard exit",
  });

  await browser.keys("ArrowRight");
  await browser.keys("ArrowRight");
  await browser.keys("Enter");
  await waitFor(async () => (await panelGitStatus(browser, "inline.txt")) === "●", {
    timeout: 10_000,
    label: "inline Git stage",
  });

  const inlineUnstage = await browser.$('[data-testid="git-inline-unstage"]');
  await inlineUnstage.waitForDisplayed({ timeout: 10_000 });
  if ((await inlineUnstage.getAttribute("data-tooltip-content")) !== "Unstage") throw new Error("Inline Unstage tooltip has unexpected text");
  await waitFor(async () => (await browser.$('[data-testid="git-inline-unstage"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git focus after stage",
  });
  await browser.keys("Enter");
  await waitFor(async () => (await panelGitStatus(browser, "inline.txt")) === "M", {
    timeout: 10_000,
    label: "inline Git unstage",
  });
  await waitFor(async () => (await browser.$('[data-testid="git-inline-stage"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git focus after unstage",
  });

  inlineRollback = await browser.$('[data-testid="git-inline-rollback"]');
  await inlineRollback.waitForDisplayed({ timeout: 10_000 });
  await browser.keys("ArrowRight");
  await browser.keys("Enter");
  inlineRollback = await browser.$('[data-testid="git-inline-rollback"]');
  await waitFor(async () => (await inlineRollback.getAttribute("aria-label")) === "Confirm rollback inline.txt", {
    timeout: 10_000,
    label: "inline Git rollback confirmation",
  });
  if ((await inlineRollback.getAttribute("data-tooltip-content")) !== "Confirm rollback") throw new Error("Inline rollback confirmation tooltip has unexpected text");
  if (fs.readFileSync(path.join(hostRepo, "inline.txt"), "utf8") !== "after inline\n") {
    throw new Error("Inline Git rollback changed the file before confirmation");
  }
  await browser.keys("Enter");
  await waitFor(async () => (await panelGitStatus(browser, "inline.txt")) === null, {
    timeout: 10_000,
    label: "inline Git rollback",
  });
  if (fs.readFileSync(path.join(hostRepo, "inline.txt"), "utf8") !== "before inline\n") {
    throw new Error("Inline Git rollback did not restore the committed content");
  }
  await waitFor(async () => (await browser.$('[data-testid="panel-left"]')).isFocused(), {
    timeout: 10_000,
    label: "panel focus after inline Git rollback",
  });

  await clickEntry(browser, "left", "nested");
  const directoryBubbleState = await waitFor(async () => {
    const state = await browser.execute(() => {
      const row = document.querySelector('[data-entry-name="nested"]');
      return { summary: !!row?.querySelector(".panel__git-bubble"), actions: !!row?.querySelector('[data-testid="git-inline-bubble"]') };
    });
    return state.summary ? state : false;
  }, { timeout: 10_000, label: "directory Git summary bubble" });
  if (!directoryBubbleState.summary || directoryBubbleState.actions) {
    throw new Error(`Directory Git bubble should remain informational: ${JSON.stringify(directoryBubbleState)}`);
  }

  const gitButton = await browser.$('[data-testid="panel-left"] .panel__git-btn');
  await gitButton.waitForClickable({ timeout: 10_000 });
  await gitButton.click();
  const gitModal = await browser.$(".git-modal");
  await gitModal.waitForDisplayed({ timeout: 10_000 });
  await (await browser.$('[data-testid="git-tab-log"]')).click();
  await waitFor(async () => (await browser.$('[data-testid="git-log-row"]')).isDisplayed(), {
    timeout: 10_000,
    label: "Git commit log",
  });
  const firstCommitSubject = await browser.execute(() => document.querySelector('[data-testid="git-log-row"] .git-modal__log-subject')?.textContent?.trim());
  if (firstCommitSubject !== "initial") {
    throw new Error(`Unexpected first Git commit: ${firstCommitSubject}`);
  }
  await (await browser.$('[data-testid="git-log-row"]')).doubleClick();
  const expandedActions = await browser.$('[data-testid="git-log-commit-show"] .git-modal__change-actions');
  await expandedActions.waitForDisplayed({ timeout: 10_000 });
  const expandedActionLabels = await browser.execute(() => [...document.querySelectorAll('[data-testid="git-log-commit-show"] .git-modal__change-action')]
    .map((button) => ({ text: button.textContent?.trim(), label: button.getAttribute("aria-label"), iconOnly: button.classList.contains("git-modal__change-action--icon") })));
  if (expandedActionLabels.length === 0 || expandedActionLabels.some((action) => action.text || !action.label || !action.iconOnly)) {
    throw new Error(`Expanded Git log actions are not icon-only: ${JSON.stringify(expandedActionLabels)}`);
  }
  await (await browser.$('[data-testid="git-tab-changes"]')).click();
  await browser.execute(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "f",
      code: "KeyF",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    }));
  });
  if (await browser.$(".search-modal__tabs").isExisting()) {
    throw new Error("Search opened above the Git modal");
  }
  if ((await browser.$$('[data-modal-state="top"]')).length !== 1
    || !await browser.$('[data-modal-id^="git:"][data-modal-state="top"]').isExisting()) {
    throw new Error("Git modal was not the sole active modal layer");
  }
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
  await browser.execute(() => document.querySelector('[data-testid="git-change-row"]')?.focus());
  await browser.keys(["Control", "a"]);
  await waitFor(async () => (await gitSelectionCount(browser)) === `${changeCount} selected`, {
    timeout: 10_000,
    label: "Git Ctrl+A select all",
  });
  await (await browser.$('[data-testid="git-clear-selection"]')).click();

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

  // Hold a Git action open so the in-flight guard can be observed without
  // depending on remote/network timing.
  const refresh = await browser.$('[data-testid="git-refresh"]');
  await refresh.waitForClickable({ timeout: 10_000 });
  await browser.execute(() => {
    window.__fbroOriginalWsSend = WebSocket.prototype.send;
    window.__fbroHeldGitRefreshCount = 0;
    WebSocket.prototype.send = function holdGitRefresh(data) {
      if (typeof data === "string") {
        try {
          const message = JSON.parse(data);
          if (message.type === "Intent" && message.name === "GitRefresh") {
            window.__fbroHeldGitRefreshCount += 1;
            return;
          }
        } catch {
          // Forward non-JSON WebSocket traffic unchanged.
        }
      }
      return window.__fbroOriginalWsSend.call(this, data);
    };
  });
  await refresh.click();
  await waitFor(async () => !(await refresh.isEnabled()), {
    timeout: 10_000,
    label: "Git action disabled while in flight",
  });
  await browser.keys("r");
  const heldRefreshCount = await browser.execute(() => window.__fbroHeldGitRefreshCount);
  await browser.execute(() => {
    WebSocket.prototype.send = window.__fbroOriginalWsSend;
    delete window.__fbroOriginalWsSend;
    delete window.__fbroHeldGitRefreshCount;
  });
  if (heldRefreshCount !== 1) {
    throw new Error(`In-flight Git refresh was sent ${heldRefreshCount} times`);
  }

  await browser.keys("Escape");
  await gitModal.waitForExist({ reverse: true, timeout: 10_000 });

  await clickEntry(browser, "left", "changed.txt");
  const inlineViewDiff = await browser.$('[data-entry-name="changed.txt"] [data-testid="git-inline-view-diff"]');
  await inlineViewDiff.waitForClickable({ timeout: 10_000 });
  await browser.keys("ArrowRight");
  await waitFor(async () => (await browser.$('[data-entry-name="changed.txt"] [data-testid="git-inline-view-diff"]')).isFocused(), {
    timeout: 10_000,
    label: "inline Git diff keyboard focus",
  });
  await browser.keys("Enter");

  observer.sendIntent("GitStagePaths", undefined, { panelId: "left", paths: [] });
  observer.sendIntent("GitViewDiff", undefined, { panelId: "left" });
  observer.sendIntent("NavigateToSource", "left", { sourceId: "journal" });
  let lastJournalRefresh = 0;
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
      "git-view-diff_v1_completed_",
      "git-view-diff_v1_failed_",
    ];
    if (Date.now() - lastJournalRefresh >= 500) {
      lastJournalRefresh = Date.now();
      observer.sendIntent("RefreshPanels");
    }
    return expectedPrefixes.every((prefix) => names.some((name) => name.startsWith(prefix)));
  }, { timeout: 10_000, label: "semantic Git journal events" });

  const localSourceId = observer.sourceIdForPath(repo);
  if (!localSourceId) throw new Error(`No local source contains ${repo}`);
  observer.sendIntent("NavigateToSource", "left", { sourceId: localSourceId });
  await waitFor(() => observer.state.panels.left?.sourceId === localSourceId, {
    timeout: 10_000,
    label: "restore local source after Git journal assertions",
  });
  await observer.waitForPanelPath("left", repo);
  await observer.waitForPanelReady("left");

  // A semantic check with unchanged Git state must not rescan the panel.
  const unchangedStart = observer.messages.length;
  observer.sendIntent("RefreshGitStatus", undefined, { panelId: "left" });
  observer.sendIntent("RequestStateSnapshot");
  await waitFor(() => observer.messages.slice(unchangedStart).some((message) => message.type === "StateSnapshot"), {
    timeout: 10_000,
    label: "unchanged Git status reconciliation",
  });
  const unchangedPanelPatches = observer.messages.slice(unchangedStart).filter((message) =>
    message.type === "StatePatch" && message.domain === "panels" && message.entityId === "left"
  );
  if (unchangedPanelPatches.length !== 0) {
    throw new Error(`Unchanged Git status caused ${unchangedPanelPatches.length} panel patches`);
  }

  // Git metadata changes do not necessarily touch visible working-tree files.
  // They remain cached until a meaningful user action requests reconciliation.
  git(hostRepo, ["add", "-A"]);
  git(hostRepo, ["commit", "-qm", "external watcher refresh"]);
  await new Promise((resolve) => setTimeout(resolve, 750));
  if (!(observer.state.panels.left?.git?.changes.length > 0)) {
    throw new Error("External commit refreshed Git state without a user trigger");
  }
  observer.sendIntent("RefreshGitStatus", undefined, { panelId: "left" });
  await waitFor(() => {
    const panel = observer.state.panels.left;
    return panel?.path === repo && panel.status === "ready" && panel.git?.changes.length === 0;
  }, { timeout: 10_000, label: "user-driven Git refresh after external commit" });

  // Navigation always performs a full scan, so leaving and returning to a
  // repository is also an explicit Git refresh trigger.
  git(hostRepo, ["branch", "navigation-refresh"]);
  observer.navigatePanelToPath("left", path.dirname(repo));
  await observer.waitForPanelPath("left", path.dirname(repo));
  observer.navigatePanelToPath("left", repo);
  await observer.waitForPanelPath("left", repo);
  await waitFor(() => observer.state.panels.left?.git?.branches.includes("navigation-refresh"), {
    timeout: 10_000,
    label: "Git refresh after navigating back to repository",
  });
}

async function panelGitStatus(browser, name) {
  return browser.execute((name) => {
    const row = [...document.querySelectorAll('[data-testid="panel-entry"]')]
      .find(candidate => candidate.getAttribute("data-entry-name") === name);
    return row?.querySelector(".panel__git-status")?.textContent ?? null;
  }, name);
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
