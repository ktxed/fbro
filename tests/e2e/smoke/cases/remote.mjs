import fs from "node:fs";
import path from "node:path";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import { reloadBrowserApp } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

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
