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
  reloadBrowserApp,
} from "../browser.mjs";
import { waitFor } from "../wait.mjs";

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

  const sharedTarget = observerA.state.shareConfig.shareSuggestions
    ?.find((target) => target.origin === "home" && target.path === aHome);
  if (!sharedTarget) throw new Error("no Home share target suggestion");
  const sharedTargetId = sharedTarget.id;

  observerA.sendIntent("SetSharedTargets", undefined, { targetIds: [sharedTargetId] });
  observerA.sendIntent("EnableSharing", undefined, { enabled: true });
  observerB.sendIntent("EnableSharing", undefined, { enabled: true });
  await waitFor(() => observerA.state.shareConfig.enabled, { timeout: 20_000, label: "fbro-a sharing enabled" });
  await waitFor(() => observerB.state.shareConfig.enabled, { timeout: 20_000, label: "fbro-b sharing enabled" });

  observerB.sendIntent("StartPairing", undefined, { address: "fbro-a:7880" });
  await Promise.all([
    observerA.confirmPairingRequests(),
    observerB.confirmPairingRequests(),
  ]);
  await waitFor(() => observerB.state.shareConfig.pairedPeers?.length > 0, {
    timeout: 20_000,
    label: "fbro-b paired peer",
  });
  const fingerprint = observerB.state.shareConfig.pairedPeers[0].fingerprint;
  observerB.sendIntent("RetryLastPeer", undefined, { fingerprint });
  await waitFor(() => Object.values(observerB.state.sources).some((source) => source.kind === "remote"), {
    timeout: 30_000,
    label: "remote source mounted",
  });
  const remoteSource = Object.values(observerB.state.sources)
    .find((source) => source.kind === "remote" && source.id.endsWith(`:${sharedTargetId}`));
  if (!remoteSource) throw new Error("remote Home source did not mount");
  if (!remoteSource.capabilities?.readOnly) throw new Error("remote source is not marked read-only");
  if (!observerB.state.panels.left?.supportsThumbnails && observerB.state.panels.left?.sourceId === remoteSource.id) {
    throw new Error("remote panel unexpectedly lacks thumbnail support");
  }
  if (remoteSource.supports.includes("DeleteSelection") || remoteSource.supports.includes("NewFolder")) {
    throw new Error("remote source exposes write capabilities");
  }

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
  if (!broOptions.bros?.some((bro) => bro.id === "builtin.sync_bro")) {
    throw new Error("Sync Bro was not offered for local/remote panel pair");
  }
  observerB.sendIntent("LaunchBro", undefined, {
    requestId: broRequestId,
    broId: "builtin.sync_bro",
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

  await browserA.getTitle();
  await browserB.getTitle();
}
