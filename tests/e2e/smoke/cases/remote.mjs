import fs from "node:fs";
import path from "node:path";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import {
  clickCommand,
  clickEntry,
  clickSource,
  closeTerminalJobDialog,
  confirmOperation,
  doubleClickEntry,
  expectCheckboxSelected,
  pressWindowKey,
  reloadBrowserApp,
  setTextInputValue,
} from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

function tinyBmp() {
  const width = 2;
  const height = 2;
  const rowSize = 8; // 2 pixels * 3 bytes, padded to a 4-byte boundary.
  const pixelBytes = rowSize * height;
  const bytes = Buffer.alloc(14 + 40 + pixelBytes);
  bytes.write("BM", 0, "ascii");
  bytes.writeUInt32LE(bytes.length, 2);
  bytes.writeUInt32LE(14 + 40, 10);
  bytes.writeUInt32LE(40, 14);
  bytes.writeInt32LE(width, 18);
  bytes.writeInt32LE(height, 22);
  bytes.writeUInt16LE(1, 26);
  bytes.writeUInt16LE(24, 28);
  bytes.writeUInt32LE(pixelBytes, 34);
  // BMP stores rows bottom-up, BGR.
  bytes.set([
    0x00, 0x00, 0xff, 0x00, 0xff, 0x00, 0x00, 0x00,
    0xff, 0x00, 0x00, 0xff, 0xff, 0xff, 0x00, 0x00,
  ], 54);
  return bytes;
}

export async function runRemoteFlow(browserA, observerA, browserB, observerB) {
  const bDst = backendPath("b", "dst");
  const bSync = backendPath("b", "sync-bro");
  const aHome = "/state/a/home";
  const aHomeDownloads = path.posix.join(aHome, "Downloads");
  const aHomeDownloadsHost = path.join(artifactsDir, "fbro-a/state/home/Downloads");
  const aHomeDeepHost = path.join(artifactsDir, "fbro-a/state/home/Projects/Deep");
  const aHomeSyncHost = path.join(artifactsDir, "fbro-a/state/home/SyncBro");
  const bSyncHost = hostPath(bSync);
  observerA.navigatePanelToPath("left", aHome);
  await observerA.waitForPanelPath("left", aHome);
  await observerA.waitForPanelNotShadow("left");
  await reloadBrowserApp(browserA, "fbro-a");
  fs.mkdirSync(aHomeDownloadsHost, { recursive: true });
  fs.mkdirSync(aHomeDeepHost, { recursive: true });
  fs.mkdirSync(aHomeSyncHost, { recursive: true });
  fs.mkdirSync(bSyncHost, { recursive: true });
  // The compose command wipes src/dst but not sync-bro; clear any stale report
  // from a previous run so the no-auto-write assertion is meaningful.
  fs.rmSync(path.join(bSyncHost, "fbro-sync-report.txt"), { force: true });
  fs.writeFileSync(path.join(aHomeDownloadsHost, "remote.txt"), "remote payload\n");
  fs.writeFileSync(path.join(aHomeDownloadsHost, "remote-thumb.bmp"), tinyBmp());
  fs.writeFileSync(path.join(aHomeDeepHost, "deep.txt"), "deep remote payload\n");
  fs.writeFileSync(path.join(aHomeSyncHost, "sync.txt"), "sync payload\n");
  fs.writeFileSync(path.join(bSyncHost, "sync.txt"), "sync payload\n");
  observerA.sendIntent("Refresh", "left");
  await observerA.waitForEntry("left", "Downloads", "directory");
  await observerA.waitForEntry("left", "Projects", "directory");
  await observerA.waitForEntry("left", "SyncBro", "directory");

  // Exercise the folder widget before enabling sharing, including keyboard
  // submission, rejection, removal, history, and both colour schemes.
  await browserA.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await waitFor(() => observerA.state.focus === "left", { timeout: 10_000, label: "sharing opener panel active" });
  await pressWindowKey(browserA, ",", { ctrl: true });
  const settingsA = await browserA.$('[data-modal-id="settings"]');
  await settingsA.waitForDisplayed({ timeout: 10_000 });
  await (await browserA.$('[data-testid="settings-tab-sharing"]')).click();
  const folderInput = await browserA.$('[data-testid="sharing-folder-path"]');
  const suggestions = await browserA.$('[data-testid="sharing-folder-suggestions"]');
  await suggestions.selectByAttribute("value", "current");
  if (await folderInput.getValue() !== aHome) throw new Error("Current folder suggestion used the wrong panel path");
  await folderInput.click();
  await browserA.keys("Enter");
  const sharedTarget = await waitFor(() => observerA.state.shareConfig.sharedRoots
    .find((target) => target.origin === "folder" && target.path === aHome),
  { timeout: 10_000, label: "current folder shared through UI" });
  const sharedTargetId = sharedTarget.id;
  await waitFor(async () => await folderInput.getValue() === "", { timeout: 10_000, label: "accepted folder input clears" });
  await expectModalFocus(browserA, '[data-testid="sharing-folder-path"]', "folder Enter preserves input focus");
  if (observerA.state.sources[sharedTarget.sourceId]) throw new Error("folder share created a panel source");

  // Add a canonical equivalent; it must remain a single share.
  await setTextInputValue(browserA, '[data-testid="sharing-folder-path"]', `${aHome}/.`);
  await browserA.keys("Enter");
  await waitFor(async () => await folderInput.getValue() === "", { timeout: 10_000, label: "duplicate folder accepted" });
  if (observerA.state.shareConfig.sharedRoots.length !== 1) throw new Error("canonical-equivalent share was duplicated");

  const typedFolder = path.posix.join(aHome, "Projects/Deep");
  for (const scheme of ["fbro-dark", "daylight"]) {
    observerA.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
    await waitFor(() => browserA.execute((scheme) => document.querySelector(".app")?.dataset.colorScheme === scheme, scheme),
      { timeout: 10_000, label: `sharing theme ${scheme}` });
    const rejectedPath = path.posix.join(aHomeDownloads, "remote.txt");
    await setTextInputValue(browserA, '[data-testid="sharing-folder-path"]', rejectedPath);
    await folderInput.click();
    await browserA.keys("Enter");
    await (await browserA.$('[data-testid="sharing-folder-error"]')).waitForDisplayed({ timeout: 10_000 });
    if (await folderInput.getValue() !== rejectedPath) throw new Error("rejected folder path was discarded");
    await expectModalFocus(browserA, '[data-testid="sharing-folder-path"]', "rejected folder preserves focus");
    if (observerA.state.shareConfig.sharedRoots.length !== 1) throw new Error("rejection changed sharing state");

    await setTextInputValue(browserA, '[data-testid="sharing-folder-path"]', typedFolder);
    const patchStart = observerA.messages.length;
    await (await browserA.$('[data-command-id="sharing.folder.add"]')).click();
    const typedTarget = await waitFor(() => observerA.state.shareConfig.sharedRoots.find((target) => target.path === typedFolder),
      { timeout: 10_000, label: "typed arbitrary folder added" });
    await expectModalFocus(browserA, '[data-testid="sharing-folder-path"]', "Add returns focus to input");
    const patches = observerA.messages.slice(patchStart).filter((message) => message.type === "StatePatch" && message.domain === "shareConfig");
    if (patches.length !== 1) throw new Error(`folder Add submitted ${patches.length} times`);
    const checkbox = await browserA.$(`[data-share-target-id="${typedTarget.id}"] input`);
    await checkbox.click();
    await waitFor(() => !observerA.state.shareConfig.sharedRoots.some((target) => target.id === typedTarget.id),
      { timeout: 10_000, label: "folder unchecked and removed" });
    await expectModalFocus(browserA, '[data-testid="sharing-folder-path"]', "removed folder returns focus to input");
    await browserA.saveScreenshot(path.join(artifactsDir, "screenshots", `sharing-folders-${scheme}.png`));
  }
  await browserA.keys(["Alt", "ArrowDown", "Alt"]);
  await (await browserA.$(".input-history")).waitForDisplayed({ timeout: 10_000 });
  await browserA.keys("Escape");
  if (!(await settingsA.isDisplayed())) throw new Error("history Escape closed Settings");
  await browserA.keys("Escape");
  await settingsA.waitForDisplayed({ timeout: 10_000, reverse: true });
  await waitFor(() => browserA.execute(() => document.activeElement === document.querySelector('[data-testid="panel-left"]')),
    { timeout: 10_000, label: "Sharing restores originating listing focus" });

  observerA.sendIntent("EnableSharing", undefined, { enabled: true });
  observerB.sendIntent("EnableSharing", undefined, { enabled: true });
  await waitFor(() => observerA.state.shareConfig.enabled, { timeout: 20_000, label: "fbro-a sharing enabled" });
  await waitFor(() => observerB.state.shareConfig.enabled, { timeout: 20_000, label: "fbro-b sharing enabled" });

  await pressWindowKey(browserB, ",", { ctrl: true });
  const settingsB = await browserB.$('[data-modal-id="settings"]');
  await settingsB.waitForDisplayed({ timeout: 10_000 });
  await (await browserB.$('[data-testid="settings-tab-sharing"]')).click();
  await (await browserB.$('[data-testid="direct-pair-address"]')).setValue("fbro-a:7880");
  await (await browserB.$('[data-command-id="sharing.pair-direct"]')).click();

  await settingsA.waitForDisplayed({ timeout: 20_000 });
  const confirmations = await Promise.all([
    browserA.$('[data-testid="pairing-confirmation"]'),
    browserB.$('[data-testid="pairing-confirmation"]'),
  ]);
  await Promise.all(confirmations.map((confirmation) => confirmation.waitForDisplayed({ timeout: 20_000 })));
  for (const browser of [browserA, browserB]) {
    await expectModalFocus(browser, '[data-command-id="pairing.cancel"]', "pairing defaults to cancel");
    const placementCorrect = await browser.execute(() => {
      const confirmation = document.querySelector('[data-testid="pairing-confirmation"]');
      if (!(confirmation instanceof HTMLElement)) return false;
      const fingerprint = confirmation.dataset.peerFingerprint;
      const matchingPeer = [...document.querySelectorAll('[data-peer-id]')]
        .find((row) => row instanceof HTMLElement && row.dataset.peerFingerprint === fingerprint);
      if (matchingPeer) return matchingPeer.contains(confirmation);
      return document.querySelector('[data-testid="direct-pair-card"]')?.contains(confirmation) ?? false;
    });
    if (!placementCorrect) throw new Error("pairing confirmation was not placed with its peer or direct-pair fallback");
    if (await browser.$('[data-modal-id="pairing"]').isExisting()) {
      throw new Error("legacy pairing modal is still present");
    }
  }
  await Promise.all([
    browserA.$('[data-command-id="pairing.confirm"]').then((button) => button.click()),
    browserB.$('[data-command-id="pairing.confirm"]').then((button) => button.click()),
  ]);
  await waitFor(() => observerB.state.shareConfig.pairedPeers?.length > 0, {
    timeout: 20_000,
    label: "fbro-b paired peer",
  });
  if (!(await settingsA.isDisplayed()) || !(await settingsB.isDisplayed())) {
    throw new Error("Sharing settings did not remain open after pairing resolved");
  }
  await Promise.all([browserA.keys("Escape"), browserB.keys("Escape")]);
  await Promise.all([
    settingsA.waitForDisplayed({ timeout: 10_000, reverse: true }),
    settingsB.waitForDisplayed({ timeout: 10_000, reverse: true }),
  ]);
  const fingerprint = observerB.state.shareConfig.pairedPeers[0].fingerprint;
  observerB.sendIntent("RetryLastPeer", undefined, { fingerprint });
  await waitFor(() => Object.values(observerB.state.sources).some((source) => source.kind === "remote"), {
    timeout: 30_000,
    label: "remote source mounted",
  });
  const remoteSource = Object.values(observerB.state.sources)
    .find((source) => source.kind === "remote" && source.id.endsWith(`:${sharedTargetId}`));
  if (!remoteSource) throw new Error("remote arbitrary folder source did not mount");
  if (!remoteSource.capabilities?.readOnly) throw new Error("remote source is not marked read-only");
  if (!observerB.state.panels.left?.supportsThumbnails && observerB.state.panels.left?.sourceId === remoteSource.id) {
    throw new Error("remote panel unexpectedly lacks thumbnail support");
  }
  if (remoteSource.supports.includes("DeleteSelection") || remoteSource.supports.includes("NewFolder")) {
    throw new Error("remote source exposes write capabilities");
  }
  await waitFor(() => observerA.state.notifications.entries.some((entry) =>
    entry.message.startsWith("Remote fbro connected:") && entry.message.endsWith("(incoming)")
    && entry.channels.includes("os")),
  { timeout: 10_000, label: "incoming remote connection notification recorded" });
  await waitFor(() => observerB.state.notifications.entries.some((entry) =>
    entry.message.startsWith("Remote fbro connected:") && entry.message.endsWith("(outgoing)")
    && entry.channels.includes("os")),
  { timeout: 10_000, label: "outgoing remote connection notification recorded" });

  observerB.sendIntent("NavigateToSource", "left", { sourceId: remoteSource.id });
  await waitFor(() => observerB.state.panels.left?.sourceId === remoteSource.id, {
    timeout: 15_000,
    label: "b left uses remote source",
  });
  const projectsEntry = await observerB.waitForEntry("left", "Projects", "directory", 20_000);
  observerB.sendIntent("SetCursor", "left", { index: observerB.state.panels.left.entries.indexOf(projectsEntry) });
  observerB.sendIntent("OpenEntry", "left");
  await observerB.waitForPanelPath("left", "Projects");
  const deepEntry = await observerB.waitForEntry("left", "Deep", "directory", 20_000);
  observerB.sendIntent("SetCursor", "left", { index: observerB.state.panels.left.entries.indexOf(deepEntry) });
  observerB.sendIntent("OpenEntry", "left");
  await observerB.waitForPanelPath("left", "Projects/Deep");
  await observerB.waitForEntry("left", "deep.txt", "file", 20_000);
  observerB.sendIntent("NavigateUp", "left");
  await observerB.waitForPanelPath("left", "Projects");
  observerB.sendIntent("NavigateToSource", "left", { sourceId: remoteSource.id });
  await observerB.waitForPanelPath("left", "");
  const downloadsEntry = await observerB.waitForEntry("left", "Downloads", "directory", 20_000);
  const downloadsIndex = observerB.state.panels.left.entries.indexOf(downloadsEntry);
  observerB.sendIntent("SetCursor", "left", { index: downloadsIndex });
  observerB.sendIntent("OpenEntry", "left");
  await observerB.waitForPanelPath("left", "Downloads");
  observerB.sendIntent("NavigatePanelTo", undefined, { panelId: "right", path: bDst });
  await observerB.waitForEntry("left", "remote.txt", "file", 20_000);
  // Exercise the viewer's full-image and partial-range reads while keeping
  // the shared remote browsing session connected.
  for (const [filename, offset, size] of [
    ["remote-thumb.bmp", 0, 4096],
    ["remote.txt", 1, 4],
  ]) {
    const viewerPath = `Downloads/${filename}`;
    const messageStart = observerB.messages.length;
    const jobsBefore = new Set(Object.keys(observerA.state.jobs));
    observerB.sendIntent("ReadFileChunk", undefined, {
      sourceId: remoteSource.id, path: viewerPath, offset, size,
    });
    await waitFor(() => observerB.messages.slice(messageStart).some((message) =>
      message.type === "FileChunk" && message.path === viewerPath
      && message.offset === offset && message.data.length > 0),
    { timeout: 20_000, label: `remote viewer read ${filename}` });
    const servingJob = await waitFor(() => Object.values(observerA.state.jobs).find((job) =>
      job.kind === "outgoing-transfer" && !jobsBefore.has(job.jobId)),
    { timeout: 10_000, label: `remote viewer outgoing job ${filename}` });
    await waitFor(() => servingJob.state === "Completed",
      { timeout: 10_000, label: `remote viewer idle completion ${filename}` });
    if (!observerB.state.sources[remoteSource.id]) throw new Error("viewer completion disconnected the remote source");
  }
  const thumbEntry = await observerB.waitForEntry("left", "remote-thumb.bmp", "file", 20_000);
  if (!observerB.state.panels.left?.supportsThumbnails) {
    throw new Error("remote Downloads panel does not advertise thumbnail support");
  }
  observerB.sendIntent("SetPanelViewMode", "left", { mode: "grid" });
  await waitFor(() => observerB.state.panels.left?.viewMode === "grid", {
    timeout: 10_000,
    label: "remote panel grid mode",
  });
  const remoteThumbUrl = `http://127.0.0.1:${observerB.state.fileServerPort}/__remote_thumb__/${encodeURIComponent(observerB.state.panels.left.sourceId)}/160/${encodeURIComponent(thumbEntry.mtime ?? "0")}/${encodeURIComponent(String(thumbEntry.size ?? 0))}/${encodeURIComponent("Downloads/remote-thumb.bmp")}`;
  await waitFor(async () => {
    const response = await fetch(remoteThumbUrl);
    if (!response.ok) return false;
    const bytes = new Uint8Array(await response.arrayBuffer());
    return bytes.length > 8
      && bytes[0] === 0x89
      && bytes[1] === 0x50
      && bytes[2] === 0x4e
      && bytes[3] === 0x47;
  }, {
    timeout: 20_000,
    interval: 250,
    label: "remote thumbnail PNG fetched through local proxy",
  });
  observerB.sendIntent("SetPanelViewMode", "left", { mode: "list" });
  await waitFor(() => observerB.state.panels.left?.viewMode === "list", {
    timeout: 10_000,
    label: "remote panel list mode restore",
  });
  await clickSource(browserB, "left", remoteSource.id);
  await doubleClickEntry(browserB, "left", "Downloads");
  await clickEntry(browserB, "left", "remote.txt");
  await clickCommand(browserB, "file.copy");
  const saveDigestFile = await expectCheckboxSelected(
    browserB,
    '[data-testid="copy-save-digest-file-input"]',
    true,
    "remote copy digest checkbox default",
  );
  await saveDigestFile.click();
  await confirmOperation(browserB, "file.copy.confirm");
  await observerB.waitForJobIdle(30_000);
  await waitFor(() => fs.existsSync(hostPath(path.posix.join(bDst, "remote.txt"))), {
    timeout: 20_000,
    label: "remote copied file on host",
  });
  const copied = fs.readFileSync(hostPath(path.posix.join(bDst, "remote.txt")), "utf8");
  if (copied !== "remote payload\n") throw new Error("remote copied file content mismatch");
  await closeTerminalJobDialog(browserB);

  // Cancel at the serving instance while the receiver has auto-resume enabled.
  // A sparse file keeps the transfer active without allocating a large fixture.
  observerB.sendIntent("SetGeneralOption", undefined, { key: "autoResumeCopies", value: true });
  await waitFor(() => observerB.state.settings.general.autoResumeCopies === true, {
    timeout: 10_000, label: "remote auto-resume enabled for cancellation regression",
  });
  const cancelName = "cancel-transfer.bin";
  const nextName = "after-cancel.txt";
  const cancelSize = 2 * 1024 * 1024 * 1024;
  const cancelFixture = path.join(aHomeDownloadsHost, cancelName);
  const fixtureFd = fs.openSync(cancelFixture, "w");
  try { fs.ftruncateSync(fixtureFd, cancelSize); } finally { fs.closeSync(fixtureFd); }
  fs.writeFileSync(path.join(aHomeDownloadsHost, nextName), "next transfer payload\n");
  observerB.sendIntent("Refresh", "left");
  await observerB.waitForEntry("left", cancelName, "file", 20_000);
  await observerB.waitForEntry("left", nextName, "file", 20_000);
  await browserA.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await browserA.keys("j");
  const jobsModal = await browserA.$('[data-modal-id="jobs"]');
  await jobsModal.waitForDisplayed({ timeout: 10_000 });
  const coveredCursor = observerA.state.panels.left.cursor;
  await browserA.keys("ArrowDown");
  if (observerA.state.panels.left.cursor !== coveredCursor) throw new Error("Jobs modal allowed covered panel navigation");
  const startCopy = (name, size) => observerB.sendIntent("StartFileOp", undefined, {
    opType: "copy", files: [{ name, kind: "file", size }], sourcePanel: "left", destPanel: "right",
    destPath: bDst, conflictPolicy: "overwrite", verificationPolicy: "prompt", saveDigestFile: false,
    background: true,
  });
  startCopy(cancelName, cancelSize);
  const incoming = await waitFor(() => Object.values(observerB.state.jobs).find(job =>
    job.kind === "file-op" && job.files?.some(file => file.name === cancelName)),
  { timeout: 10_000, label: "remote copy started" });
  const outgoing = await waitFor(() => Object.values(observerA.state.jobs).find(job =>
    job.kind === "outgoing-transfer" && job.state === "Running"
    && job.progress?.currentFile?.endsWith(cancelName) && job.progress.totalBytesCopied > 0),
  { timeout: 10_000, label: "outgoing transfer actively serving bytes" });
  const cancelSelector = `[data-job-id="${outgoing.jobId}"] [data-testid="job-cancel"]`;
  await browserA.$(cancelSelector).waitForDisplayed({ timeout: 10_000 });
  let reachedCancel = false;
  for (let index = 0; index < 40 && !reachedCancel; index++) {
    await browserA.keys("Tab");
    reachedCancel = await browserA.execute(selector => document.activeElement === document.querySelector(selector), cancelSelector);
  }
  if (!reachedCancel) throw new Error("Outgoing cancellation is unreachable by keyboard");
  await browserA.keys("Enter");
  await waitFor(() => observerA.state.jobs[outgoing.jobId]?.state === "Cancelled" && observerB.state.jobs[incoming.jobId]?.state === "Cancelled", {
    timeout: 15_000, label: "both sides acknowledge outgoing cancellation",
  });
  startCopy(nextName, "next transfer payload\n".length);
  const next = await waitFor(() => Object.values(observerB.state.jobs).find(job =>
    job.state === "Completed" && job.files?.some(file => file.name === nextName)),
  { timeout: 20_000, label: "next remote copy completes after cancellation" });
  if (next.error) throw new Error(`Next remote copy failed: ${next.error}`);
  if (fs.readFileSync(hostPath(path.posix.join(bDst, nextName)), "utf8") !== "next transfer payload\n") {
    throw new Error("Copy after cancellation has incorrect content");
  }
  if (fs.existsSync(hostPath(path.posix.join(bDst, cancelName)))) throw new Error("Cancelled fresh transfer left an unwanted partial");
  await browserA.keys("Escape");
  await jobsModal.waitForDisplayed({ timeout: 10_000, reverse: true });
  await waitFor(() => browserA.execute(() => document.activeElement === document.querySelector('[data-testid="panel-left"]')), {
    timeout: 10_000, label: "outgoing cancellation returns focus to its opener",
  });
  const cancelledBytes = observerA.state.jobs[outgoing.jobId].progress.totalBytesCopied;
  // Wait past the outgoing completion timer and ensure neither job revives.
  const stableUntil = Date.now() + 2_000;
  await waitFor(() => {
    if (observerA.state.jobs[outgoing.jobId]?.state !== "Cancelled" || observerB.state.jobs[incoming.jobId]?.state !== "Cancelled" || observerA.state.jobs[outgoing.jobId].progress.totalBytesCopied !== cancelledBytes) {
      throw new Error("Cancelled transfer revived or continued serving bytes");
    }
    return Date.now() >= stableUntil;
  }, { timeout: 5_000, label: "cancelled transfer remains terminal" });
  observerB.sendIntent("Refresh", "left");
  await observerB.waitForEntry("left", "remote.txt", "file", 10_000);
  fs.unlinkSync(cancelFixture);

  observerB.sendIntent("NavigateToSource", "left", { sourceId: remoteSource.id });
  await waitFor(() => {
    const panel = observerB.state.panels.left;
    const index = panel?.entries?.findIndex((entry) =>
      entry.name === "SyncBro" && entry.kind === "directory"
    ) ?? -1;
    return panel?.sourceId === remoteSource.id
      && panel.path === ""
      && panel.status === "ready"
      && index >= 0
      ? index + 1
      : false;
  }, {
    timeout: 20_000,
    label: "remote root SyncBro entry ready",
  });
  observerB.sendIntent("NavigateToHistoryEntry", undefined, {
    panelId: "left",
    sourceId: remoteSource.id,
    path: "SyncBro",
  });
  await observerB.waitForPanelPath("left", "SyncBro");
  await observerB.waitForEntry("left", "sync.txt", "file", 20_000);
  observerB.navigatePanelToPath("right", bSync);
  await observerB.waitForPanelPath("right", bSync);
  await observerB.waitForEntry("right", "sync.txt", "file", 20_000);

  const broRequestId = `sync-bro-${Date.now()}`;
  observerB.sendIntent("ListBros", "left", { requestId: broRequestId });
  const broOptions = await observerB.waitForBroOptions(broRequestId, 20_000);
  if (!broOptions.bros?.some((bro) => bro.id === "builtin:sync-bro")) {
    throw new Error("Sync Bro was not offered for local/remote panel pair");
  }
  observerB.sendIntent("LaunchBro", undefined, {
    requestId: broRequestId,
    broId: "builtin:sync-bro",
  });
  const syncJob = await observerB.waitForJob("sync-bro", "Completed", 45_000);
  const remoteManifestJob = await observerA.waitForJob("remote-manifest", "Completed", 45_000);
  if (syncJob.error) throw new Error(`Sync Bro failed: ${syncJob.error}`);
  if (remoteManifestJob.error) throw new Error(`Remote manifest failed: ${remoteManifestJob.error}`);

  // Results are delivered to the client for the modal, not written to disk.
  const result = await waitFor(() => syncJob.syncResult ?? false, {
    timeout: 10_000,
    label: "Sync Bro result delivered to client",
  });
  if (result.matched !== 1) {
    throw new Error(`Sync Bro matched ${result.matched}, expected 1`);
  }
  if (
    result.differing.length
    || result.localOnly.length
    || result.remoteOnly.length
    || result.unverified.length
    || result.localErrors.length
    || result.remoteErrors.length
  ) {
    throw new Error(`Sync Bro reported unexpected differences: ${JSON.stringify(result)}`);
  }
  const reportPath = path.join(bSyncHost, "fbro-sync-report.txt");
  if (fs.existsSync(reportPath)) {
    throw new Error("Sync Bro wrote a report file without an explicit save");
  }

  // The report file is written only on demand via SaveSyncReport.
  observerB.sendIntent("SaveSyncReport", undefined, {
    jobId: syncJob.jobId,
    result: syncJob.syncResult,
  });
  await waitFor(() => fs.existsSync(reportPath), {
    timeout: 10_000,
    label: "Sync Bro report saved on demand",
  });
  const report = fs.readFileSync(reportPath, "utf8");
  if (!report.includes("Matched: 1")) {
    throw new Error(`Sync Bro report did not include the expected match count:\n${report}`);
  }
  await waitFor(() => syncJob.reportSavedPath ?? false, {
    timeout: 10_000,
    label: "Sync Bro report-saved confirmation",
  });

  observerB.sendIntent("DisconnectRemotePeer", undefined, { sourceId: remoteSource.id });
  await waitFor(() => !observerB.state.sources[remoteSource.id],
    { timeout: 10_000, label: "explicit remote disconnect removes mounted source" });
  await waitFor(() => observerB.state.notifications.entries.some((entry) =>
    entry.message.startsWith("Remote fbro disconnected:") && entry.message.endsWith("(outgoing)")
    && entry.channels.includes("os")),
  { timeout: 10_000, label: "outgoing remote disconnection notification recorded" });
  await waitFor(() => observerA.state.notifications.entries.some((entry) =>
    entry.message.startsWith("Remote fbro disconnected:") && entry.message.endsWith("(incoming)")
    && entry.channels.includes("os")),
  { timeout: 10_000, label: "incoming remote disconnection notification recorded" });

  await browserA.getTitle();
  await browserB.getTitle();
}
