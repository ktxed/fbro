import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import { clickEntry, goToPanelPath, setTextInputValue } from "../browser.mjs";
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
  const initialBranch = execFileSync("git", ["-C", hostRepo, "branch", "--show-current"], { encoding: "utf8" }).trim();
  git(hostRepo, ["branch", "feature/Fix-login"]);
  git(hostRepo, ["branch", "release/fix-logging"]);
  for (let index = 0; index < 24; index++) git(hostRepo, ["branch", `picker-scroll-${String(index).padStart(2, "0")}`]);
  git(hostRepo, ["switch", "-qc", "feature/picker-rejected"]);
  fs.writeFileSync(path.join(hostRepo, "changed.txt"), "conflicting branch\n");
  git(hostRepo, ["commit", "-qam", "conflicting branch"]);
  git(hostRepo, ["switch", "-q", initialBranch]);
  git(hostRepo, ["remote", "add", "origin", "."]);
  git(hostRepo, ["update-ref", "refs/remotes/origin/remote-topic", "HEAD"]);
  git(hostRepo, ["update-ref", "refs/remotes/origin/remote-name-conflict", "HEAD"]);
  git(hostRepo, ["update-ref", "refs/remotes/origin/remote-rejected", "feature/picker-rejected"]);
  git(hostRepo, ["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/remote-topic"]);
  git(hostRepo, ["branch", "remote-name-conflict"]);
  git(hostRepo, ["branch", "remote-namespace-conflict"]);
  git(hostRepo, ["branch", "origin/remote-topic"]);
  git(hostRepo, ["update-ref", "refs/remotes/origin/remote-namespace-conflict/nested", "HEAD"]);
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
  // Inline branch creation must keep editing keys inside the prompt.
  await browser.keys("n");
  const branchInput = await browser.$('[data-testid="git-branch-name"]');
  await waitFor(() => branchInput.isFocused(), { timeout: 5_000, label: "new branch input focus" });
  await browser.keys("Escape");
  await waitFor(async () => await (await browser.$('[data-testid="git-new-branch"]')).isFocused(), {
    timeout: 5_000, label: "branch cancel focus restoration",
  });
  await browser.keys("n");
  await branchInput.setValue("bad..name");
  await browser.keys("Enter");
  await waitFor(async () => await (await browser.$('.git-modal__branch-error')).isDisplayed(), {
    timeout: 10_000, label: "invalid branch name error",
  });
  if (await branchInput.getValue() !== "bad..name") throw new Error("Branch error discarded the name");
  const statusBeforeBranch = execFileSync("git", ["-C", hostRepo, "status", "--porcelain"], { encoding: "utf8" });
  await setTextInputValue(browser, '[data-testid="git-branch-name"]', "feature/smoke-new-branch");
  await browser.keys("Enter");
  await waitFor(() => observer.state.panels.left?.git?.branch === "feature/smoke-new-branch", {
    timeout: 10_000, label: "created branch backend refresh",
  });
  await waitFor(async () => !await (await browser.$('[data-testid="git-branch-form"]')).isExisting(), {
    timeout: 5_000, label: "branch form closes after success",
  });
  const branchSelect = await browser.$('[data-testid="git-branch-picker"]');
  if (await branchSelect.getAttribute("data-branch") !== "feature/smoke-new-branch" || !await branchSelect.isFocused()) {
    throw new Error("New branch selector did not update and receive focus");
  }
  if (execFileSync("git", ["-C", hostRepo, "status", "--porcelain"], { encoding: "utf8" }) !== statusBeforeBranch) {
    throw new Error("Branch creation changed uncommitted work");
  }
  await runGitBranchPickerFlow(browser, observer);
  await runGitRemoteBranchFlow(browser, observer, hostRepo);

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
  await browser.keys("j");
  await waitFor(async () => (await browser.$('[data-testid="git-change-row"][data-change-index="1"]')).isFocused(), {
    timeout: 10_000,
    label: "Git change row J moves down",
  });
  if (observer.state.panels.left?.cursor !== panelCursor) {
    throw new Error("Git modal J navigation changed the panel cursor");
  }
  await browser.keys("k");
  await waitFor(async () => (await browser.$('[data-testid="git-change-row"][data-change-index="0"]')).isFocused(), {
    timeout: 10_000,
    label: "Git change row K moves up",
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
  const busyPicker = await browser.$('[data-testid="git-branch-picker"]');
  if (await busyPicker.getAttribute("aria-disabled") !== "true") throw new Error("Branch picker was enabled during Git activity");
  await busyPicker.click();
  await browser.keys("ArrowDown");
  if (await browser.$('[data-testid="git-branch-dropdown"]').isExisting()) throw new Error("Busy branch picker opened");
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
  await waitFor(() => browser.execute(() =>
    document.querySelector('[data-testid="panel-left"] .panel__git-btn') === document.activeElement),
  { timeout: 5_000, label: "Git modal restores exact repository button opener" });

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
  await runGitLogTabFlow(browser, observer, repo, hostRepo);
}

async function runGitLogTabFlow(browser, observer, repo, hostRepo) {
  // Overflow both the viewport and the first history page. The newest commit
  // has message pagination and file actions as well as branch/tag copy buttons.
  for (let index = 0; index < 105; index++) git(hostRepo, ["commit", "--allow-empty", "-qm", `history ${index}`]);
  fs.writeFileSync(path.join(hostRepo, "log-tab.txt"), "keyboard navigation\n");
  fs.writeFileSync(path.join(hostRepo, "log-tab-second.txt"), "sibling row\n");
  git(hostRepo, ["add", "log-tab.txt", "log-tab-second.txt"]);
  git(hostRepo, ["commit", "-qm", ["Tab navigation", "", ...Array.from({ length: 17 }, (_, index) => `Message line ${index}`)].join("\n")]);
  git(hostRepo, ["tag", "tab-navigation"]);
  await goToPanelPath(browser, observer, "left", path.dirname(repo));
  await goToPanelPath(browser, observer, "left", repo);
  await (await browser.$('[data-testid="panel-left"] .panel__git-btn')).click();
  const panel = '[data-testid="git-log-panel"]';
  const row = '[data-testid="git-log-row"]';
  const firstRow = `${row}[data-log-index="0"]`;
  await waitFor(() => browser.execute(() => document.querySelector('[data-testid="git-log-row"] .git-modal__log-subject')?.textContent === "Tab navigation"), { timeout: 10_000, label: "Git refreshed history" });
  await waitFor(() => browser.$(firstRow).isFocused(), { timeout: 10_000, label: "Git log initial row focus" });

  const expectFocus = async (selector, label) => waitFor(() => browser.$(selector).isFocused(), { timeout: 5_000, label });
  const expectTabExit = async (selector) => {
    await expectFocus(selector, "Git active item focus before Tab");
    const before = await browser.execute((panel) => document.querySelector(panel).scrollTop, panel);
    const buttons = await browser.execute((selector) => document.querySelector(selector).querySelectorAll("button:not([disabled])").length, selector);
    for (let index = 0; index < buttons; index++) {
      await browser.keys("Tab");
      if (!await browser.execute((selector) => document.querySelector(selector).contains(document.activeElement), selector)) {
        throw new Error("Tab skipped an active Git row button");
      }
    }
    await browser.keys("Tab");
    const state = await browser.execute((panel) => {
      const list = document.querySelector(panel);
      return { inside: list.contains(document.activeElement), scrollTop: list.scrollTop };
    }, panel);
    if (state.inside || Math.abs(state.scrollTop - before) > 1) {
      throw new Error(`Git Tab did not leave the active row without scrolling: ${JSON.stringify({ before, ...state })}`);
    }
    // Reverse through the same buttons to reach the row itself again.
    for (let index = 0; index <= buttons; index++) await browser.keys(["Shift", "Tab"]);
    await expectFocus(selector, "Git Shift+Tab returns to active row");
    await browser.keys(["Shift", "Tab"]);
    if (await browser.execute((panel) => document.querySelector(panel).contains(document.activeElement), panel)) {
      throw new Error("Git Shift+Tab entered an inactive row");
    }
    await browser.keys("Tab");
    await expectFocus(selector, "Git Tab re-enters active row");
  };

  await expectTabExit(firstRow);
  for (const [key, index] of [["ArrowDown", 1], ["j", 2], ["k", 1], ["ArrowUp", 0]]) {
    await browser.keys(key);
    await expectFocus(`${row}[data-log-index="${index}"]`, `Git log ${key} focus`);
  }
  await browser.keys("PageDown");
  await waitFor(() => browser.execute(() => Number(document.activeElement?.getAttribute("data-log-index")) > 0), { timeout: 5_000, label: "Git PageDown moves row focus" });
  const pageRow = await browser.execute(() => document.activeElement.getAttribute("data-log-index"));
  await expectTabExit(`${row}[data-log-index="${pageRow}"]`);
  await browser.keys("PageUp");
  await expectFocus(firstRow, "Git PageUp returns to first row");

  await browser.keys("ArrowRight");
  const message = ".git-modal__log-message-load-more";
  await expectFocus(message, "Git long commit message focus");
  await expectTabExit(message);
  await browser.keys("Space");
  await expectFocus(message, "Git message has another page");
  await browser.keys("Enter");
  const change = '[data-testid="git-log-change-row"][data-change-index="0"]';
  await expectFocus(change, "Git fully loaded message hands focus to file row");
  await expectTabExit(change);
  await browser.keys("ArrowDown");
  await expectTabExit('[data-testid="git-log-change-row"][data-change-index="1"]');
  await browser.keys("ArrowLeft");
  await expectFocus(firstRow, "Git file row returns to commit");
  await browser.keys("ArrowLeft");

  // Clicking a previously inactive row must also update its Tab participation.
  const clickedRow = `${row}[data-log-index="3"]`;
  await browser.$(clickedRow).click();
  await expectTabExit(clickedRow);
  await browser.keys("End");
  const lastIndex = await browser.execute((row) => document.querySelectorAll(row).length - 1, row);
  await expectTabExit(`${row}[data-log-index="${lastIndex}"]`);
  await browser.keys("Home");
  await expectFocus(firstRow, "Git Home returns to first row");
  const count = await browser.$$(row).then(rows => rows.length);
  await browser.keys("Space");
  await waitFor(async () => (await browser.$$(row)).length > count && !await browser.$('[data-testid="git-log-load-more"]').isExisting(), { timeout: 10_000, label: "Git Space loads remaining history" });
  await expectFocus(firstRow, "Git pagination preserves active row");
  await expectTabExit(firstRow);
  await browser.keys("Escape");
  await expectFocus('[data-testid="panel-left"] .panel__git-btn', "Git log close restores its opener");
}

async function runGitBranchPickerFlow(browser, observer) {
  const pickerSelector = '[data-testid="git-branch-picker"]';
  const searchSelector = '[data-testid="git-branch-search"]';
  const popupSelector = '[data-testid="git-branch-dropdown"]';
  const picker = await browser.$(pickerSelector);
  const originalBranch = observer.state.panels.left.git.branch;
  const originalScheme = observer.state.settings.general.colorScheme;
  const cursor = observer.state.panels.left.cursor;
  const optionNames = () => browser.execute(() => [...document.querySelectorAll('[data-testid="git-branch-option"]')].map(option => option.textContent.trim()));
  const activeName = () => browser.execute(() => document.querySelector('[data-testid="git-branch-option"][data-active="true"]')?.textContent.trim());
  const sentCount = () => browser.execute(() => window.__pickerSwitches.length);
  const expectClosed = () => waitFor(async () => !await browser.$(popupSelector).isExisting(), { timeout: 5_000, label: "branch dropdown dismissed" });
  const expectPickerFocus = () => waitFor(() => picker.isFocused(), { timeout: 5_000, label: "branch picker focus restored" });
  const open = async (key = "Enter") => {
    await browser.keys("b");
    await expectPickerFocus();
    await browser.keys(key);
    await waitFor(async () => await browser.$(searchSelector).isFocused(), { timeout: 5_000, label: "branch search input focus" });
  };
  await browser.execute(() => {
    window.__pickerOriginalSend = WebSocket.prototype.send;
    window.__pickerSwitches = [];
    window.__pickerHold = false;
    WebSocket.prototype.send = function(data) {
      const message = typeof data === "string" ? JSON.parse(data) : null;
      if (message?.type === "Intent" && message.name === "GitSwitchBranch") {
        window.__pickerSwitches.push(message);
        if (window.__pickerHold) {
          window.__pickerHeldSend = { socket: this, data };
          return;
        }
      }
      return window.__pickerOriginalSend.call(this, data);
    };
  });
  try {
    await open();
    for (const scheme of ["fbro-dark", "daylight"]) {
      observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
      await waitFor(() => browser.execute(scheme => document.querySelector('[data-testid="git-branch-dropdown"]')?.getAttribute("data-color-scheme") === scheme, scheme), { timeout: 5_000, label: `branch dropdown ${scheme} theme` });
      const contained = await browser.execute(() => {
        const popup = document.querySelector('[data-testid="git-branch-dropdown"]');
        const bounds = popup.getBoundingClientRect();
        return !!popup.closest('[data-modal-state="top"]') && bounds.left >= 0 && bounds.top >= 0 && bounds.right <= window.innerWidth && bounds.bottom <= window.innerHeight;
      });
      if (!contained) throw new Error(`Branch dropdown escaped its modal or viewport in ${scheme}`);
      await browser.saveScreenshot(path.join(artifactsDir, "screenshots", `git-branch-picker-${scheme}.png`));
    }
    await browser.keys("End");
    const activeVisible = await browser.execute(() => {
      const row = document.querySelector('[data-testid="git-branch-option"][data-active="true"]').getBoundingClientRect();
      const list = document.querySelector('.git-branch-picker__list').getBoundingClientRect();
      return row.top >= list.top - 1 && row.bottom <= list.bottom + 1;
    });
    if (!activeVisible) throw new Error("Active branch was not scrolled into view");
    await setTextInputValue(browser, searchSelector, "FIX");
    const expected = ["feature/Fix-login", "release/fix-logging"];
    if ((await optionNames()).join("|") !== expected.join("|")) throw new Error("Branch search did not match case-insensitive substrings");
    for (const [key, name] of [["End", expected[1]], ["ArrowUp", expected[0]], ["ArrowDown", expected[1]], ["Home", expected[0]]]) {
      await browser.keys(key);
      if (await activeName() !== name || !await browser.$(searchSelector).isFocused()) throw new Error(`Branch navigation failed for ${key}`);
    }
    if (observer.state.panels.left.cursor !== cursor || await sentCount() !== 0) throw new Error("Branch search/navigation changed backend selection");
    await setTextInputValue(browser, searchSelector, "no-such-branch");
    await browser.keys("ArrowDown");
    await browser.keys("Enter");
    if ((await optionNames()).length !== 0 || !await browser.$(`${popupSelector} [role="status"]`).isDisplayed() || await sentCount() !== 0) throw new Error("Empty branch search submitted a switch");
    await setTextInputValue(browser, searchSelector, "");
    if ((await optionNames()).length !== (observer.state.panels.left.git.branches.length + observer.state.panels.left.git.remoteBranches.length)) throw new Error("Clearing branch search did not restore the list");
    await browser.keys("Escape");
    await expectClosed();
    await expectPickerFocus();
    if (!await browser.$(".git-modal").isDisplayed()) throw new Error("Branch search Escape closed the Git modal");

    await open("ArrowDown");
    if (await browser.$(searchSelector).getValue() !== "") throw new Error("Branch search query survived reopening");
    await browser.keys("Tab");
    await expectClosed();
    if (await picker.isFocused()) throw new Error("Branch dropdown took focus back after Tab");
    await open("Space");
    await browser.keys(["Shift", "Tab"]);
    await expectClosed();

    await open();
    await browser.$(".git-modal__eyebrow").click();
    await expectClosed();
    if (!await browser.execute(() => document.querySelector('[data-modal-id="git:left"]').contains(document.activeElement))) throw new Error("Outside dismissal lost Git modal focus");
    await open();
    await browser.$('[data-testid="git-new-branch"]').click();
    await expectClosed();
    await waitFor(async () => await browser.$('[data-testid="git-branch-name"]').isFocused(), { timeout: 5_000, label: "outside click keeps successor input focus" });
    await browser.keys("Escape");

    await open();
    await setTextInputValue(browser, searchSelector, originalBranch);
    await browser.keys("Enter");
    await expectClosed();
    await expectPickerFocus();
    if (await sentCount() !== 0) throw new Error("Choosing the current branch sent a switch");

    await open();
    await setTextInputValue(browser, searchSelector, "Fix-login");
    await browser.execute((selector) => {
      window.__pickerHold = true;
      const input = document.querySelector(selector);
      for (let count = 0; count < 2; count++) input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    }, searchSelector);
    await expectClosed();
    await expectPickerFocus();
    if (await sentCount() !== 1 || await picker.getAttribute("aria-disabled") !== "true") throw new Error("Branch switch did not guard duplicate/busy submissions");
    if (await picker.getAttribute("data-branch") !== originalBranch || observer.state.panels.left.git.branch !== originalBranch) throw new Error("Branch picker optimistically changed the branch");
    await picker.click();
    await browser.keys("ArrowDown");
    if (await browser.$(popupSelector).isExisting()) throw new Error("Picker reopened during checkout");
    await browser.execute(() => {
      window.__pickerHold = false;
      const held = window.__pickerHeldSend;
      window.__pickerOriginalSend.call(held.socket, held.data);
    });
    await waitFor(async () => observer.state.panels.left.git.branch === "feature/Fix-login" && await picker.getAttribute("data-branch") === "feature/Fix-login" && await picker.getAttribute("aria-disabled") === "false", { timeout: 10_000, label: "backend-confirmed branch switch" });

    await picker.click();
    await waitFor(async () => await browser.$(searchSelector).isFocused(), { timeout: 5_000, label: "pointer opens branch search" });
    await setTextInputValue(browser, searchSelector, "fix-logging");
    await browser.$('[data-testid="git-branch-option"]').click();
    await expectClosed();
    await waitFor(async () => observer.state.panels.left.git.branch === "release/fix-logging" && await picker.getAttribute("aria-disabled") === "false", { timeout: 10_000, label: "pointer branch selection" });
    await expectPickerFocus();

    await open();
    await setTextInputValue(browser, searchSelector, "picker-rejected");
    await browser.keys("Enter");
    await expectClosed();
    await waitFor(async () => await picker.getAttribute("aria-disabled") === "false" && /overwritten|commit your changes/i.test(await browser.$(".git-modal__activity").getText()), { timeout: 10_000, label: "rejected branch switch feedback" });
    if (observer.state.panels.left.git.branch !== "release/fix-logging" || await picker.getAttribute("data-branch") !== "release/fix-logging") throw new Error("Rejected checkout changed the picker value");
    await expectPickerFocus();
    if (await sentCount() !== 3) throw new Error("Unexpected branch switch count");
    await open();
    await setTextInputValue(browser, searchSelector, "fix");
    await browser.execute(() => [...document.querySelectorAll('[data-testid="git-branch-option"]')].find(option => option.textContent.trim() === "release/fix-logging").focus());
    await browser.keys("Enter");
    await expectClosed();
    await expectPickerFocus();
    if (await sentCount() !== 3) throw new Error("Enter activated the highlight instead of the directly focused branch option");
  } finally {
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: originalScheme });
    await browser.execute(() => {
      WebSocket.prototype.send = window.__pickerOriginalSend;
      delete window.__pickerOriginalSend;
      delete window.__pickerSwitches;
      delete window.__pickerHold;
      delete window.__pickerHeldSend;
    });
  }
}

async function runGitRemoteBranchFlow(browser, observer, hostRepo) {
  const picker = await browser.$('[data-testid="git-branch-picker"]');
  const original = observer.state.panels.left.git.branch;
  const select = async (name, pointer = false, kind = "remote") => {
    await picker.click();
    await waitFor(async () => await browser.$('[data-testid="git-branch-search"]').isFocused(), { timeout: 5_000, label: "remote branch search focus" });
    const groups = await browser.execute(() => [...document.querySelectorAll('.git-branch-picker__group-label')].map(el => el.textContent));
    if (groups.join("|") !== "Local|Remote") throw new Error("Branch groups are missing");
    if (await browser.$('[data-testid="git-branch-option"][data-ref="refs/remotes/origin/HEAD"]').isExisting()) throw new Error("Remote HEAD alias was offered for checkout");
    await setTextInputValue(browser, '[data-testid="git-branch-search"]', name);
    if (pointer) await browser.$(`[data-testid="git-branch-option"][data-kind="${kind}"]`).click();
    else {
      // A local branch may have the same display name; the remote group is last.
      await browser.keys(kind === "local" ? "Home" : "End");
      await browser.keys("Enter");
    }
  };
  const expectBranch = async name => {
    await waitFor(async () => observer.state.panels.left.git.branch === name && await picker.getAttribute("aria-disabled") === "false", { timeout: 10_000, label: `checkout ${name}` });
    await waitFor(() => picker.isFocused(), { timeout: 5_000, label: "remote checkout focus return" });
  };
  await select("origin/remote-topic");
  await expectBranch("remote-topic");
  const upstream = () => execFileSync("git", ["-C", hostRepo, "rev-parse", "--symbolic-full-name", "@{upstream}"], { encoding: "utf8" }).trim();
  if (upstream() !== "refs/remotes/origin/remote-topic" || observer.state.panels.left.git.detached) throw new Error("Remote checkout did not create a tracking branch");
  await select(original);
  await expectBranch(original);
  await select("origin/remote-topic", true);
  await expectBranch("remote-topic");

  await select("origin/remote-topic", true, "local");
  await expectBranch("origin/remote-topic");
  await select("origin/remote-topic");
  await expectBranch("remote-topic");

  await select("origin/remote-namespace-conflict/nested");
  const input = await browser.$('[data-testid="git-branch-name"]');
  await waitFor(() => input.isFocused(), { timeout: 5_000, label: "remote conflict name input" });
  await browser.keys("Escape");
  await waitFor(() => picker.isFocused(), { timeout: 5_000, label: "remote conflict cancellation" });
  if (observer.state.panels.left.git.branch !== "remote-topic") throw new Error("Cancelling remote conflict changed HEAD");
  await select("origin/remote-name-conflict");
  await waitFor(() => input.isFocused(), { timeout: 5_000, label: "remote conflict retry input" });
  await setTextInputValue(browser, '[data-testid="git-branch-name"]', "bad..name");
  await browser.keys("Enter");
  await waitFor(async () => await browser.$('.git-modal__branch-error').isDisplayed() && await input.isEnabled(), { timeout: 10_000, label: "remote name validation failure" });
  if (await input.getValue() !== "bad..name") throw new Error("Remote checkout error discarded the entered name");
  await setTextInputValue(browser, '[data-testid="git-branch-name"]', "remote-chosen");
  await browser.keys("Enter");
  await expectBranch("remote-chosen");
  if (upstream() !== "refs/remotes/origin/remote-name-conflict") throw new Error("Named remote checkout has wrong upstream");
  await select("origin/remote-rejected");
  await waitFor(async () => await picker.getAttribute("aria-disabled") === "false" && /overwritten|commit your changes/i.test(await browser.$('.git-modal__activity').getText()), { timeout: 10_000, label: "remote checkout rejection" });
  if (observer.state.panels.left.git.branch !== "remote-chosen") throw new Error("Rejected remote checkout changed HEAD");
  await waitFor(() => picker.isFocused(), { timeout: 5_000, label: "rejected remote checkout focus" });
  await select(original);
  await expectBranch(original);
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
