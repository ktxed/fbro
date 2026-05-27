import fs from "node:fs";
import path from "node:path";
import WebSocket from "ws";
import { artifactsDir } from "./config.mjs";
import { waitFor } from "./wait.mjs";

export class FbroObserver {
  constructor(name, url) {
    this.name = name;
    this.url = url;
    this.ws = null;
    this.messages = [];
    this.state = {
      panels: {},
      sources: {},
      jobs: {},
      shareConfig: { enabled: false, allowIpv6: false, sharedSourceIds: [], pairedPeers: [] },
      discoveredPeers: [],
      focus: "",
    };
    this.pairingRequests = new Map();
  }

  async connect() {
    await waitFor(() => new Promise((resolve) => {
      const ws = new WebSocket(this.url);
      const timer = setTimeout(() => {
        ws.terminate();
        resolve(false);
      }, 800);
      ws.once("open", () => {
        clearTimeout(timer);
        this.ws = ws;
        ws.on("message", (data) => this.#onMessage(String(data)));
        ws.send(JSON.stringify({
          type: "Hello",
          timestamp: Date.now(),
          protocolVersion: "0",
          client: { kind: "gui", name: `fbro-e2e-${this.name}`, supports: ["jobs", "decisions"] },
        }));
        resolve(true);
      });
      ws.once("error", () => {
        clearTimeout(timer);
        resolve(false);
      });
    }), { timeout: 30_000, interval: 250, label: `${this.name} observer websocket` });

    await this.waitForPanelReady("left", 45_000);
    await this.waitForPanelReady("right", 45_000);
  }

  close() {
    this.ws?.close();
    fs.writeFileSync(
      path.join(artifactsDir, "transcripts", `${this.name}.json`),
      JSON.stringify(this.messages, null, 2),
    );
  }

  sendIntent(name, scope, args = {}, options = {}) {
    this.ws.send(JSON.stringify({
      type: "Intent",
      timestamp: Date.now(),
      name,
      scope,
      args,
      options,
    }));
  }

  sourceIdForPath(targetPath) {
    return Object.values(this.state.sources)
      .filter((source) =>
        source.kind === "filesystem"
        && source.mounted
        && !source.shadow
        && source.rootPath
        && (targetPath === source.rootPath || targetPath.startsWith(`${source.rootPath}/`))
      )
      .sort((a, b) => b.rootPath.length - a.rootPath.length)[0]?.id;
  }

  navigatePanelToPath(panelId, targetPath, cursorHint) {
    const sourceId = this.sourceIdForPath(targetPath);
    if (sourceId) {
      this.sendIntent("NavigateToHistoryEntry", undefined, { panelId, path: targetPath, sourceId });
      return;
    }
    this.sendIntent("NavigatePanelTo", undefined, { panelId, path: targetPath, cursorHint });
  }

  waitForPanelReady(panelId, timeout = 15_000) {
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      return panel?.status === "ready" ? panel : false;
    }, { timeout, label: `${this.name} panel ${panelId} ready` });
  }

  waitForEntry(panelId, entryName, kind, timeout = 15_000) {
    let lastSnapshotRequest = 0;
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      const entry = panel?.entries?.find((candidate) => candidate.name === entryName);
      if (entry && (!kind || entry.kind === kind)) return entry;
      const now = Date.now();
      if (now - lastSnapshotRequest > 500) {
        lastSnapshotRequest = now;
        this.sendIntent("RequestStateSnapshot");
      }
      return false;
    }, { timeout, label: `${this.name} ${panelId} entry ${entryName}` });
  }

  async waitForAllEntries(panelId, names, timeout = 15_000) {
    for (const name of names) {
      await this.waitForEntry(panelId, name, undefined, timeout);
    }
  }

  waitForSort(panelId, expectedBy, expectedOrder, timeout = 15_000) {
    let lastSnapshotRequest = 0;
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      if (panel?.sort?.by === expectedBy && panel?.sort?.order === expectedOrder) return panel;
      const now = Date.now();
      if (now - lastSnapshotRequest > 500) {
        lastSnapshotRequest = now;
        this.sendIntent("RequestStateSnapshot");
      }
      return false;
    }, { timeout, label: `${this.name} ${panelId} sort ${expectedBy} ${expectedOrder}` });
  }

  assertFileEntryOrder(panelId, expectedNames) {
    const panel = this.state.panels[panelId];
    if (!panel?.entries) throw new Error(`panel ${panelId} has no entries`);
    const fileNames = panel.entries
      .filter((e) => e.kind !== "directory")
      .map((e) => e.name);
    if (fileNames.length !== expectedNames.length) {
      throw new Error(
        `panel ${panelId}: expected ${expectedNames.length} file entries but got ${fileNames.length} (${fileNames.join(", ")})`,
      );
    }
    for (let i = 0; i < expectedNames.length; i++) {
      if (fileNames[i] !== expectedNames[i]) {
        throw new Error(
          `panel ${panelId}: at position ${i} expected "${expectedNames[i]}" but got "${fileNames[i]}" (got [${fileNames.join(", ")}])`,
        );
      }
    }
  }

  waitForNoEntry(panelId, entryName, timeout = 15_000) {
    let lastSnapshotRequest = 0;
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      if (panel && !panel.entries?.some((candidate) => candidate.name === entryName)) return panel;
      const now = Date.now();
      if (now - lastSnapshotRequest > 500) {
        lastSnapshotRequest = now;
        this.sendIntent("RequestStateSnapshot");
      }
      return false;
    }, { timeout, label: `${this.name} ${panelId} no entry ${entryName}` });
  }

  waitForPanelPath(panelId, expectedPath, timeout = 15_000) {
    let lastSnapshotRequest = 0;
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      if (panel?.path === expectedPath) return panel;
      const now = Date.now();
      if (now - lastSnapshotRequest > 500) {
        lastSnapshotRequest = now;
        this.sendIntent("RequestStateSnapshot");
      }
      return false;
    }, { timeout, label: `${this.name} ${panelId} path ${expectedPath}` });
  }

  waitForPanelShadow(panelId, timeout = 20_000) {
    let lastSnapshotRequest = 0;
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      if (panel?.shadowSource) return panel.shadowSource;
      const now = Date.now();
      if (now - lastSnapshotRequest > 500) {
        lastSnapshotRequest = now;
        this.sendIntent("RequestStateSnapshot");
      }
      return false;
    }, { timeout, label: `${this.name} ${panelId} shadow source` });
  }

  waitForPanelNotShadow(panelId, timeout = 15_000) {
    let lastSnapshotRequest = 0;
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      if (panel && !panel.shadowSource) return panel;
      const now = Date.now();
      if (now - lastSnapshotRequest > 500) {
        lastSnapshotRequest = now;
        this.sendIntent("RequestStateSnapshot");
      }
      return false;
    }, { timeout, label: `${this.name} ${panelId} non-shadow source` });
  }

  waitForJobIdle(timeout = 30_000) {
    return waitFor(() => {
      const active = Object.values(this.state.jobs).filter((job) =>
        job.state === "Queued" || job.state === "Running" || job.state === "AwaitingDecision"
      );
      return active.length === 0;
    }, { timeout, label: `${this.name} jobs idle` });
  }

  waitForSearchResultsMounted(panelId, timeout = 10_000) {
    let lastSnapshotRequest = 0;
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      if (panel?.sourceId?.startsWith("search:")) return panel;
      const now = Date.now();
      if (now - lastSnapshotRequest > 500) {
        lastSnapshotRequest = now;
        this.sendIntent("RequestStateSnapshot");
      }
      return false;
    }, { timeout, label: `${this.name} search results mounted` });
  }

  async confirmPairingRequests() {
    await waitFor(() => this.pairingRequests.size > 0, {
      timeout: 20_000,
      label: `${this.name} pairing request`,
    });
    for (const handshakeId of this.pairingRequests.keys()) {
      this.sendIntent("ConfirmPairing", undefined, { handshakeId, accept: true });
    }
  }

  #onMessage(raw) {
    let msg;
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    this.messages.push(msg);
    if (msg.type === "StateSnapshot") this.#applySnapshot(msg.domains);
    if (msg.type === "StatePatch") this.#applyPatch(msg);
    if (msg.type === "JobStarted") {
      this.state.jobs[msg.job.jobId] = {
        ...msg.job,
        state: msg.job.initialState,
        searchResults: msg.job.kind === "search" ? [] : undefined,
        searchScanned: msg.job.kind === "search" ? 0 : undefined,
        searchMatched: msg.job.kind === "search" ? 0 : undefined,
      };
    }
    if (msg.type === "JobEvent") this.#applyJobEvent(msg);
    if (msg.type === "PairingRequested") this.pairingRequests.set(msg.handshakeId, msg);
    if (msg.type === "PairingResolved") this.pairingRequests.delete(msg.handshakeId);
    if (msg.type === "RemoteSourceStatus" && msg.status !== "connected") {
      this.state.remoteSourceStatus ??= {};
      this.state.remoteSourceStatus[msg.sourceId] = msg;
    }
    if (msg.type === "RemoteSourceStatus" && msg.status === "connected") {
      delete this.state.remoteSourceStatus?.[msg.sourceId];
    }
  }

  #applySnapshot(domains) {
    if (domains.panels) this.state.panels = domains.panels;
    if (domains.sources) {
      this.state.sources = Object.fromEntries(domains.sources.map((source) => [source.id, source]));
    }
    if (domains.jobs) this.state.jobs = Object.fromEntries(domains.jobs.map((job) => [job.jobId, job]));
    if (domains.focus !== undefined) this.state.focus = domains.focus;
    if (domains.shareConfig) this.state.shareConfig = domains.shareConfig;
    if (domains.discoveredPeers) this.state.discoveredPeers = domains.discoveredPeers;
  }

  #applyPatch(msg) {
    const patch = msg.patch ?? {};
    if (msg.domain === "panels" && msg.entityId) {
      const current = this.state.panels[msg.entityId] ?? {};
      const next = { ...current, ...patch };
      if (patch.appendEntries) next.entries = [...(current.entries ?? []), ...patch.appendEntries];
      if (patch.windowEntries) next.entries = patch.windowEntries;
      if (patch.shadowSource === null) delete next.shadowSource;
      this.state.panels[msg.entityId] = next;
    }
    if (msg.domain === "sources" && Array.isArray(patch.sources)) {
      this.state.sources = Object.fromEntries(patch.sources.map((source) => [source.id, source]));
    }
    if (msg.domain === "focus") this.state.focus = patch.focus;
    if (msg.domain === "shareConfig") {
      this.state.shareConfig = { ...this.state.shareConfig, ...patch };
    }
    if (msg.domain === "discoveredPeers") this.state.discoveredPeers = patch.peers ?? [];
    if (msg.domain === "jobs" && patch.removeJobId) delete this.state.jobs[patch.removeJobId];
  }

  #applyJobEvent(msg) {
    const job = this.state.jobs[msg.jobId];
    if (!job) return;
    if (msg.event === "Queued") job.state = "Queued";
    if (msg.event === "Progress") job.state = "Running";
    if (msg.event === "DecisionRequired") job.state = "AwaitingDecision";
    if (msg.event === "Completed") job.state = "Completed";
    if (msg.event === "Cancelled") job.state = "Cancelled";
    if (msg.event === "Failed") job.state = "Failed";
    if (msg.progress) job.progress = msg.progress;
    if (msg.searchProgress) {
      job.searchResults = [...(job.searchResults ?? []), ...(msg.searchProgress.newResults ?? [])];
      job.searchScanned = msg.searchProgress.scanned;
      job.searchMatched = msg.searchProgress.matched;
      job.searchCurrentPath = msg.searchProgress.currentPath;
    }
    if (msg.decision) job.decision = msg.decision;
    if (msg.error) job.error = msg.error;
    if (msg.resultPath) job.resultPath = msg.resultPath;
    if (msg.resultPanelId) job.resultPanelId = msg.resultPanelId;
  }
}
