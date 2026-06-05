import fs from "node:fs";
import path from "node:path";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import { reloadBrowserApp } from "../browser.mjs";
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
  const aHome = "/state/a/home";
  const aHomeDownloads = path.posix.join(aHome, "Downloads");
  const aHomeDownloadsHost = path.join(artifactsDir, "fbro-a/state/home/Downloads");
  const aHomeDeepHost = path.join(artifactsDir, "fbro-a/state/home/Projects/Deep");
  observerA.navigatePanelToPath("left", aHome);
  await observerA.waitForPanelPath("left", aHome);
  await observerA.waitForPanelNotShadow("left");
  await reloadBrowserApp(browserA, "fbro-a");
  fs.mkdirSync(aHomeDownloadsHost, { recursive: true });
  fs.mkdirSync(aHomeDeepHost, { recursive: true });
  fs.writeFileSync(path.join(aHomeDownloadsHost, "remote.txt"), "remote payload\n");
  fs.writeFileSync(path.join(aHomeDownloadsHost, "remote-thumb.bmp"), tinyBmp());
  fs.writeFileSync(path.join(aHomeDeepHost, "deep.txt"), "deep remote payload\n");
  observerA.sendIntent("Refresh", "left");
  await observerA.waitForEntry("left", "Downloads", "directory");
  await observerA.waitForEntry("left", "Projects", "directory");

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
  observerB.sendIntent("NavigateUp", "left");
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
  observerB.sendIntent("StartFileOp", undefined, {
    opType: "copy",
    files: [{ name: "remote.txt", kind: "file" }],
    sourcePanel: "left",
    destPanel: "right",
    destPath: bDst,
    conflictPolicy: "overwrite",
    verificationPolicy: "fail",
    saveDigestFile: false,
    background: true,
  });
  await observerB.waitForJobIdle(30_000);
  await waitFor(() => fs.existsSync(hostPath(path.posix.join(bDst, "remote.txt"))), {
    timeout: 20_000,
    label: "remote copied file on host",
  });
  const copied = fs.readFileSync(hostPath(path.posix.join(bDst, "remote.txt")), "utf8");
  if (copied !== "remote payload\n") throw new Error("remote copied file content mismatch");

  await browserA.getTitle();
  await browserB.getTitle();
}
