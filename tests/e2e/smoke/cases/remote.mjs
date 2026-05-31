import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { reloadBrowserApp } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runRemoteFlow(browserA, observerA, browserB, observerB) {
  const aSrc = backendPath("a", "src");
  const bDst = backendPath("b", "dst");
  observerA.navigatePanelToPath("left", aSrc);
  await observerA.waitForPanelPath("left", aSrc);
  await observerA.waitForPanelNotShadow("left");
  await reloadBrowserApp(browserA, "fbro-a");
  fs.writeFileSync(hostPath(path.posix.join(aSrc, "remote.txt")), "remote payload\n");
  observerA.sendIntent("Refresh", "left");
  await observerA.waitForEntry("left", "remote.txt", "file");

  const sharedSourceId = observerA.sourceIdForPath(aSrc);
  if (!sharedSourceId) throw new Error("no filesystem source to share for /work/a");
  const sharedTargetId = observerA.state.shareConfig.shareSuggestions
    ?.find((target) => target.sourceId === sharedSourceId && target.origin === "source")
    ?.id ?? sharedSourceId;

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
  if (!remoteSource) throw new Error("remote /work/a source did not mount");
  if (!remoteSource.capabilities?.readOnly) throw new Error("remote source is not marked read-only");
  if (remoteSource.supports.includes("DeleteSelection") || remoteSource.supports.includes("NewFolder")) {
    throw new Error("remote source exposes write capabilities");
  }

  observerB.sendIntent("NavigateToSource", "left", { sourceId: remoteSource.id });
  await waitFor(() => observerB.state.panels.left?.sourceId === remoteSource.id, {
    timeout: 15_000,
    label: "b left uses remote source",
  });
  const srcEntry = await observerB.waitForEntry("left", "src", "directory", 20_000);
  const srcIndex = observerB.state.panels.left.entries.indexOf(srcEntry);
  observerB.sendIntent("SetCursor", "left", { index: srcIndex });
  observerB.sendIntent("OpenEntry", "left");
  await observerB.waitForPanelPath("left", "src");
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
