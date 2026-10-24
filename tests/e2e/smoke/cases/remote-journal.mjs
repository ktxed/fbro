import path from "node:path";
import { waitFor } from "../wait.mjs";

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function requireEvent(events, predicate, label) {
  const event = events.find(predicate);
  if (!event) throw new Error(`Missing remote journal event: ${label}`);
  return event;
}

function jobEvents(events, jobId) {
  return events.filter((event) => event.details?.jobId === jobId).sort((a, b) => a.sequence - b.sequence);
}

function expectLifecycle(events, phases, label) {
  if (events.length !== phases.length || events.some((event, index) => event.phase !== phases[index])) {
    throw new Error(`${label}: expected ${phases.join(", ")}, got ${events.map((event) => event.phase).join(", ")}`);
  }
  if (new Set(events.map((event) => event.operationId)).size !== 1) {
    throw new Error(`${label}: lifecycle lost its stable operation ID`);
  }
}

async function readJournal(observer, since, expectedPrefixes) {
  observer.sendIntent("NavigateToSource", "left", { sourceId: "journal" });
  let refreshedAt = 0;
  const entries = await waitFor(() => {
    const panel = observer.state.panels.left;
    if (panel?.sourceId !== "journal" || panel.status !== "ready") return false;
    if (Date.now() - refreshedAt >= 500) {
      refreshedAt = Date.now();
      observer.sendIntent("RefreshPanels");
    }
    const currentEntries = panel.entries.filter((entry) => entry.name.startsWith("remote-")
      && Number(entry.mtime) >= Math.floor(since / 1000));
    return expectedPrefixes.every((prefix) => currentEntries.some((entry) => entry.name.startsWith(prefix)))
      ? currentEntries : false;
  }, { timeout: 15_000, label: "durable remote lifecycle journal entries" });
  const events = [];
  for (const entry of entries) {
    // Journal sourcePath is the event UUID, independent of its display filename.
    const eventPath = entry.sourcePath ?? entry.name.match(/_([0-9a-f-]{36})\.log$/i)?.[1];
    if (!eventPath || !uuid.test(eventPath)) throw new Error(`Invalid journal event filename: ${entry.name}`);
    const messageStart = observer.messages.length;
    observer.sendIntent("ReadFileChunk", undefined, {
      sourceId: "journal", path: eventPath, offset: 0, size: 1024 * 1024,
    });
    const chunk = await waitFor(() => observer.messages.slice(messageStart).find((message) =>
      message.type === "FileChunk" && message.path === eventPath && message.offset === 0),
    { timeout: 10_000, label: `read journal ${entry.name}` });
    if (!chunk.eof) throw new Error(`Journal payload exceeded smoke read size: ${entry.name}`);
    const text = Buffer.from(chunk.data, "hex").toString("utf8");
    const marker = "\nPayload:\n";
    const payloadStart = text.indexOf(marker);
    if (payloadStart < 0) throw new Error(`Journal event has no Payload section: ${entry.name}`);
    const event = JSON.parse(text.slice(payloadStart + marker.length));
    if (event.timestampMs < since) continue;
    if (!uuid.test(event.operationId) || !Number.isInteger(event.sequence) || event.sequence < 1
      || !Object.hasOwn(event, "peerFingerprint") || !Object.hasOwn(event, "peerName")
      || !event.direction || !event.details
      || entry.name !== `${event.kind}_v1_${event.phase}_${eventPath}.log`) {
      throw new Error(`Invalid remote journal envelope: ${entry.name}`);
    }
    // Secrets must never enter an event, including nested operation details.
    if (/"(?:pairingCode|code|nonce|privateKey|secretKey|publicKey|hostKey|rawKey)"\s*:/i.test(JSON.stringify(event))) {
      throw new Error(`Remote journal contains pairing/key material: ${entry.name}`);
    }
    events.push(event);
  }
  if (new Set(events.map((event) => event.sequence)).size !== events.length) {
    throw new Error("Remote journal reused an event sequence");
  }
  return events;
}

export async function verifyRemoteJournal(observer, options) {
  const original = { sourceId: observer.state.panels.left.sourceId, path: observer.state.panels.left.path };
  const fallbackPath = path.posix.dirname(options.knownFile);
  const previous = observer.state.sources[original.sourceId] ? original
    : { sourceId: observer.sourceIdForPath(fallbackPath), path: fallbackPath };
  try {
    const events = await readJournal(observer, options.since, [
      "remote-pairing_v1_requested_", "remote-pairing_v1_paired_",
      "remote-pairing_v1_revoked_",
      "remote-connection_v1_connected_", "remote-connection_v1_disconnected_",
      "remote-transfer_v1_started_", "remote-transfer_v1_cancelled_",
      `${options.completedKind}_v1_completed_`,
      ...(options.viewerJobs ? ["remote-transfer_v1_resumed_", "remote-transfer_v1_ended_"] : []),
    ]);
    const pairing = requireEvent(events, (event) => event.kind === "remote-pairing" && event.phase === "paired", "pairing completed");
    expectLifecycle(events.filter((event) => event.operationId === pairing.operationId).sort((a, b) => a.sequence - b.sequence),
      ["requested", "paired"], "pairing on each peer");
    if (options.peerFingerprint && pairing.peerFingerprint !== options.peerFingerprint) {
      throw new Error("Pairing journal contains the wrong peer fingerprint");
    }
    const revoked = requireEvent(events, (event) => event.kind === "remote-pairing" && event.phase === "revoked"
      && event.peerFingerprint === pairing.peerFingerprint, "paired peer revocation");
    if (revoked.operationId === pairing.operationId) throw new Error("Revocation reused the completed handshake operation");
    const disconnected = requireEvent(events, (event) => event.kind === "remote-connection"
      && event.phase === "disconnected" && event.direction === options.direction, `${options.direction} disconnect`);
    expectLifecycle(events.filter((event) => event.operationId === disconnected.operationId).sort((a, b) => a.sequence - b.sequence),
      ["connected", "disconnected"], "connection transition");
    expectLifecycle(jobEvents(events, options.cancelledJobId), ["started", "cancelled"], "cancelled transfer");
    expectLifecycle(jobEvents(events, options.completedJobId), ["started", "completed"], options.completedKind);
    for (const jobId of options.viewerJobs ?? []) {
      const lifecycle = jobEvents(events, jobId);
      expectLifecycle(lifecycle, ["started", "ended", "resumed", "ended"], "viewer idle and resume");
      if (!lifecycle.at(-1).details.progress || lifecycle.at(-1).details.progress.totalBytesCopied <= 0) {
        throw new Error("Viewer journal omitted actual served byte progress");
      }
    }
    if (options.keyboardJobId) {
      const ended = requireEvent(jobEvents(events, options.keyboardJobId), (event) =>
        event.phase === "ended" && event.details.files?.length === 28, "complete 28-file serving snapshot");
      for (let index = 1; index <= 28; index += 1) {
        const name = `job-file-${String(index).padStart(2, "0")}.txt`;
        if (!ended.details.files.some((file) => file.name === name
          && file.location?.path === path.posix.join(options.keyboardRoot, name))) {
          throw new Error(`Serving journal omitted backend-resolved path for ${name}`);
        }
      }
      if (Object.hasOwn(ended.details, "destPath")) throw new Error("Serving journal invented an unknown destination");
    }
    requireEvent(events, (event) => event.kind === "remote-transfer" && event.details.files?.some((file) =>
      file.location?.path === options.knownFile || file.sourcePath === options.knownFile
      || (event.details.destPath === path.posix.dirname(options.knownFile) && file.name === path.posix.basename(options.knownFile))),
    `known transfer path ${options.knownFile}`);
    if (observer.state.notifications.entries.length !== 0) {
      throw new Error("Journal inspection unexpectedly republished cleared remote notifications");
    }
  } finally {
    observer.sendIntent("NavigateToHistoryEntry", undefined, { panelId: "left", ...previous });
    await waitFor(() => observer.state.panels.left?.sourceId === previous.sourceId
      && observer.state.panels.left?.path === previous.path && observer.state.panels.left?.status === "ready",
    { timeout: 10_000, label: "restore listing after remote journal inspection" });
  }
}
