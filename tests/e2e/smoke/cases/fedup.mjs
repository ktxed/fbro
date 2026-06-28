import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { closeTerminalJobDialog, pressPanelKey, reloadBrowserApp, setTextInputValue } from "../browser.mjs";
import { cssString, waitFor } from "../wait.mjs";

const GLOBAL_SOURCE_ID = "global-selections";
const FEDUP_BRO_ID = "builtin:fedup";

export async function runFedupFlow(browser, observer) {
  const aSrc = backendPath("a", "src");
  const root = path.posix.join(aSrc, "fedup-smoke");
  const runId = Date.now().toString(36);

  fs.rmSync(hostPath(root), { recursive: true, force: true });

  const defaultIgnored = path.posix.join(root, "default-ignored");
  writeFile(path.posix.join(defaultIgnored, "plain", "a.txt"), "same default\n");
  writeFile(path.posix.join(defaultIgnored, "plain", "b.txt"), "same default\n");
  writeFile(path.posix.join(defaultIgnored, "plain", "unique.txt"), "unique default\n");
  writeFile(path.posix.join(defaultIgnored, "node_modules", "ignored-a.txt"), "ignored default\n");
  writeFile(path.posix.join(defaultIgnored, "node_modules", "ignored-b.txt"), "ignored default\n");
  writeFile(path.posix.join(defaultIgnored, ".venv", "ignored-a.py"), "ignored venv\n");
  writeFile(path.posix.join(defaultIgnored, ".venv", "ignored-b.py"), "ignored venv\n");
  writeFile(path.posix.join(defaultIgnored, "artifacts", "e2e", "state", "cache-a.bin"), "ignored artifacts\n");
  writeFile(path.posix.join(defaultIgnored, "artifacts", "e2e", "state", "cache-b.bin"), "ignored artifacts\n");

  const defaultResult = await runFedupCase(browser, observer, {
    selectionName: `Fedup defaults ${runId}`,
    inputPath: defaultIgnored,
    params: { useDefaultIgnores: "true", includePatterns: "", excludePatterns: "" },
  });
  assertResultCounts(defaultResult, {
    label: "default ignores",
    scannedFiles: 3,
    duplicateGroups: 1,
    duplicateFiles: 1,
  });
  assertNoPathIncludes(defaultResult, "/node_modules/");
  assertNoPathIncludes(defaultResult, "/.venv/");
  assertNoPathIncludes(defaultResult, "/artifacts/e2e/");

  const includeOnly = path.posix.join(root, "include-only");
  writeFile(path.posix.join(includeOnly, "video-a.mp4"), "same video\n");
  writeFile(path.posix.join(includeOnly, "video-b.MP4"), "same video\n");
  writeFile(path.posix.join(includeOnly, "note-a.txt"), "same text\n");
  writeFile(path.posix.join(includeOnly, "note-b.txt"), "same text\n");

  const includeResult = await runFedupCase(browser, observer, {
    selectionName: `Fedup include ${runId}`,
    inputPath: includeOnly,
    params: { useDefaultIgnores: "true", includePatterns: "*.mp4", excludePatterns: "" },
  });
  assertResultCounts(includeResult, {
    label: "include patterns",
    scannedFiles: 2,
    duplicateGroups: 1,
    duplicateFiles: 1,
  });
  for (const filePath of resultPaths(includeResult)) {
    if (!filePath.toLowerCase().endsWith(".mp4")) {
      throw new Error(`include patterns should only return mp4 files, got ${filePath}`);
    }
  }

  const customExclude = path.posix.join(root, "custom-exclude");
  writeFile(path.posix.join(customExclude, "keep", "a.bin"), "same keep\n");
  writeFile(path.posix.join(customExclude, "keep", "b.bin"), "same keep\n");
  writeFile(path.posix.join(customExclude, "skip-me", "a.bin"), "same skip\n");
  writeFile(path.posix.join(customExclude, "skip-me", "b.bin"), "same skip\n");

  const excludeResult = await runFedupCase(browser, observer, {
    selectionName: `Fedup exclude ${runId}`,
    inputPath: customExclude,
    params: { useDefaultIgnores: "false", includePatterns: "", excludePatterns: "skip-me" },
  });
  assertResultCounts(excludeResult, {
    label: "exclude patterns",
    scannedFiles: 2,
    duplicateGroups: 1,
    duplicateFiles: 1,
  });
  assertNoPathIncludes(excludeResult, "/skip-me/");
}

function writeFile(backendFilePath, contents) {
  const localPath = hostPath(backendFilePath);
  fs.mkdirSync(path.dirname(localPath), { recursive: true });
  fs.writeFileSync(localPath, contents);
}

async function runFedupCase(browser, observer, { selectionName, inputPath, params }) {
  await createGlobalSelection(observer, selectionName, inputPath);
  await navigateToGlobalSelections(observer);
  await reloadBrowserApp(browser, observer.name);
  await waitForPanelReadyAtPath(observer, "left", GLOBAL_SOURCE_ID, "");

  await launchFedupFromDialog(browser, observer, selectionName, params);
  const closedUiJob = await closeTerminalJobDialog(browser, 45_000);
  if (!closedUiJob) {
    throw new Error(`Fedup UI job result did not appear for ${selectionName}`);
  }

  return launchFedupWithObserver(observer, selectionName, params);
}

async function launchFedupWithObserver(observer, selectionName, params) {
  await navigateToGlobalSelections(observer);
  await waitForPanelReadyAtPath(observer, "left", GLOBAL_SOURCE_ID, "");

  const requestId = `fedup-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  observer.sendIntent("ListBros", "left", {
    requestId,
    targetPath: selectionName,
  });
  const broOptions = await observer.waitForBroOptions(requestId, 20_000);
  if (!broOptions.bros?.some((bro) => bro.id === FEDUP_BRO_ID)) {
    throw new Error(`Fedup Bro was not offered for global selection ${selectionName}`);
  }

  const existingJobIds = new Set(Object.keys(observer.state.jobs));
  observer.sendIntent("LaunchBro", undefined, {
    requestId,
    broId: FEDUP_BRO_ID,
    params,
  });
  const job = await waitFor(() => {
    return Object.values(observer.state.jobs).find((candidate) =>
      candidate.kind === "fedup-bro"
      && candidate.state === "Completed"
      && !existingJobIds.has(candidate.jobId)
    ) ?? false;
  }, { timeout: 45_000, label: `${observer.name} Fedup Bro completed for ${selectionName}` });

  return waitFor(() => job.fedupResult ?? false, {
    timeout: 10_000,
    label: `${observer.name} Fedup Bro result for ${selectionName}`,
  });
}

async function launchFedupFromDialog(browser, observer, selectionName, params) {
  const entryIndex = await waitFor(() => {
    const panel = observer.state.panels.left;
    const index = panel?.entries?.findIndex((entry) =>
      entry.name === selectionName && entry.kind === "directory"
    ) ?? -1;
    return panel?.sourceId === GLOBAL_SOURCE_ID
      && panel.path === ""
      && panel.status === "ready"
      && index >= 0
      ? index + 1
      : false;
  }, {
    timeout: 10_000,
    label: `${observer.name} global selection entry ${selectionName} ready`,
  });

  observer.sendIntent("SetCursor", "left", { index: entryIndex - 1 });
  await waitFor(() => {
    const panel = observer.state.panels.left;
    return panel?.sourceId === GLOBAL_SOURCE_ID
      && panel.path === ""
      && panel.cursor === entryIndex - 1
      ? panel
      : false;
  }, {
    timeout: 10_000,
    label: `${observer.name} cursor on ${selectionName}`,
  });
  observer.sendIntent("SwitchFocus", undefined, { to: "left" });
  await waitFor(() => observer.state.focus === "left", {
    timeout: 10_000,
    label: `${observer.name} focus left panel`,
  });
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());

  await pressPanelKey(browser, "left", "F1");
  const dialog = await browser.$('[data-testid="bro-dialog"]');
  await dialog.waitForDisplayed({ timeout: 10_000 });

  const fedupOption = await browser.$(
    `[data-testid="bro-dialog"] [data-testid="bro-option"][data-bro-id="${cssString(FEDUP_BRO_ID)}"]`,
  );
  await fedupOption.waitForClickable({ timeout: 20_000 });
  await fedupOption.click();

  await setFedupDefaultIgnores(browser, params.useDefaultIgnores !== "false");
  if (params.includePatterns) {
    await setTextInputValue(browser, '[data-testid="fedup-include-patterns-input"]', params.includePatterns);
  }
  if (params.excludePatterns) {
    await setTextInputValue(browser, '[data-testid="fedup-exclude-patterns-input"]', params.excludePatterns);
  }

  const launchButton = await browser.$('[data-testid="bro-dialog"] [data-testid="bro-launch-button"]');
  await launchButton.waitForClickable({ timeout: 10_000 });
  await launchButton.click();
  await waitFor(async () => !await browser.$('[data-testid="bro-dialog"]').then((el) => el.isExisting()), {
    timeout: 10_000,
    label: "Pass To Bro dialog closed",
  });
}

async function setFedupDefaultIgnores(browser, expected) {
  const input = await browser.$('[data-testid="fedup-default-ignores-input"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  const selected = await input.isSelected();
  if (selected === expected) return;
  await input.click();
  await waitFor(async () => await input.isSelected() === expected, {
    timeout: 5_000,
    label: `Fedup default ignores ${expected ? "checked" : "unchecked"}`,
  });
}

async function createGlobalSelection(observer, selectionName, inputPath) {
  const parentPath = path.posix.dirname(inputPath);
  const entryName = path.posix.basename(inputPath);
  await navigateLocalPath(observer, "left", parentPath);
  await observer.waitForEntry("left", entryName, "directory");
  observer.sendIntent("AddToGlobalSelection", "left", {
    panelId: "left",
    targetPath: inputPath,
    name: selectionName,
  });
  await observer.waitForGlobalSelection(selectionName, 1, 20_000);
}

async function navigateLocalPath(observer, panelId, targetPath) {
  const sourceId = observer.sourceIdForPath(targetPath);
  if (!sourceId) throw new Error(`no local filesystem source for ${targetPath}`);
  await navigateSourcePath(observer, panelId, sourceId, targetPath);
}

async function navigateToGlobalSelections(observer) {
  await observer.waitForSource(GLOBAL_SOURCE_ID, 20_000);
  await navigateSourcePath(observer, "left", GLOBAL_SOURCE_ID, "");
}

async function navigateSourcePath(observer, panelId, sourceId, targetPath) {
  if (observer.state.panels[panelId]?.sourceId !== sourceId) {
    observer.sendIntent("NavigateToSource", panelId, { sourceId });
  }
  await waitForPanelSource(observer, panelId, sourceId);
  await waitForPanelReadyOnSource(observer, panelId, sourceId);
  if (observer.state.panels[panelId]?.path !== targetPath) {
    const startIndex = observer.messages.length;
    observer.sendIntent("NavigatePanelTo", undefined, { panelId, path: targetPath });
    await waitForNavigateOk(observer, panelId, startIndex);
  }
  await waitForPanelReadyAtPath(observer, panelId, sourceId, targetPath);
}

async function waitForPanelSource(observer, panelId, sourceId) {
  return waitFor(() => {
    const panel = observer.state.panels[panelId];
    return panel?.sourceId === sourceId ? panel : false;
  }, { timeout: 10_000, label: `${observer.name} ${panelId} source ${sourceId}` });
}

async function waitForPanelReadyOnSource(observer, panelId, sourceId) {
  return waitFor(() => {
    const panel = observer.state.panels[panelId];
    return panel?.sourceId === sourceId && panel.status === "ready" ? panel : false;
  }, {
    timeout: 15_000,
    label: `${observer.name} ${panelId} ready on ${sourceId}`,
  });
}

async function waitForPanelReadyAtPath(observer, panelId, sourceId, expectedPath) {
  return waitFor(() => {
    const panel = observer.state.panels[panelId];
    return panel?.sourceId === sourceId
      && panel.path === expectedPath
      && panel.status === "ready"
      ? panel
      : false;
  }, {
    timeout: 15_000,
    label: `${observer.name} ${panelId} ready at ${expectedPath}`,
  });
}

async function waitForNavigateOk(observer, panelId, startIndex) {
  return waitFor(() => {
    return observer.messages.slice(startIndex).some((msg) =>
      msg.type === "NavigateOk" && msg.panelId === panelId
    );
  }, {
    timeout: 10_000,
    label: `${observer.name} ${panelId} navigate ok`,
  });
}

function assertResultCounts(result, { label, scannedFiles, duplicateGroups, duplicateFiles }) {
  if (result.errors.length !== 0) {
    throw new Error(`${label}: expected no scan errors, got ${JSON.stringify(result.errors)}`);
  }
  if (result.scannedFiles !== scannedFiles) {
    throw new Error(`${label}: expected ${scannedFiles} scanned files, got ${result.scannedFiles}`);
  }
  if (result.duplicateGroups !== duplicateGroups) {
    throw new Error(`${label}: expected ${duplicateGroups} duplicate groups, got ${result.duplicateGroups}`);
  }
  if (result.duplicateFiles !== duplicateFiles) {
    throw new Error(`${label}: expected ${duplicateFiles} duplicate files, got ${result.duplicateFiles}`);
  }
}

function assertNoPathIncludes(result, needle) {
  const offendingPath = resultPaths(result).find((filePath) => filePath.includes(needle));
  if (offendingPath) throw new Error(`Fedup result should not include ${needle}, got ${offendingPath}`);
}

function resultPaths(result) {
  return result.groups.flatMap((group) => [
    group.keeper.path,
    ...group.duplicates.map((duplicate) => duplicate.path),
  ]);
}
