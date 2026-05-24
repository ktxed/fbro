import { remote } from "webdriverio";
import WebSocket from "ws";
import fs from "node:fs";
import path from "node:path";
import { execFileSync, spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const localMode = process.env.FBRO_E2E_LOCAL === "1";
const artifactsDir = process.env.FBRO_E2E_ARTIFACTS ?? path.resolve("artifacts/e2e");
const appBinary = process.env.FBRO_APP_BINARY ?? (
  localMode
    ? ["src-tauri/target/release/fbro", "src-tauri/target/debug/fbro"]
        .map((candidate) => path.join(repoRoot, candidate))
        .find((candidate) => fs.existsSync(candidate))
    : "/app/src-tauri/target/release/fbro"
);
const backendRoots = { a: "/work/a", b: "/work/b" };
const hostRoots = {
  a: process.env.FBRO_A_HOST_ROOT ?? (localMode ? path.join(artifactsDir, "fbro-a/work") : backendRoots.a),
  b: process.env.FBRO_B_HOST_ROOT ?? (localMode ? path.join(artifactsDir, "fbro-b/work") : backendRoots.b),
};
const wsUrls = {
  a: process.env.FBRO_A_WS_URL ?? (localMode ? "ws://127.0.0.1:17878" : "ws://fbro-a:7878"),
  b: process.env.FBRO_B_WS_URL ?? (localMode ? "ws://127.0.0.1:27878" : "ws://fbro-b:7878"),
};
const webdriverHosts = {
  a: process.env.FBRO_A_WEBDRIVER_HOST ?? (localMode ? "127.0.0.1" : "fbro-a"),
  b: process.env.FBRO_B_WEBDRIVER_HOST ?? (localMode ? "127.0.0.1" : "fbro-b"),
};
const webdriverPorts = {
  a: Number(process.env.FBRO_A_WEBDRIVER_PORT ?? (localMode ? 4444 : 4444)),
  b: Number(process.env.FBRO_B_WEBDRIVER_PORT ?? (localMode ? 4445 : 4444)),
};

function backendPath(instance, ...parts) {
  return path.posix.join(backendRoots[instance], ...parts);
}

function hostPath(backendPathValue) {
  for (const [instance, backendRoot] of Object.entries(backendRoots)) {
    if (backendPathValue === backendRoot || backendPathValue.startsWith(`${backendRoot}/`)) {
      return path.join(hostRoots[instance], backendPathValue.slice(backendRoot.length));
    }
  }
  return backendPathValue;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function cssString(value) {
  return String(value).replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

async function waitFor(fn, { timeout = 15_000, interval = 100, label = "condition" } = {}) {
  const start = Date.now();
  let lastError;
  while (Date.now() - start < timeout) {
    try {
      const value = await fn();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await sleep(interval);
  }
  throw new Error(`Timed out waiting for ${label}${lastError ? `: ${lastError.message}` : ""}`);
}

function ensureLocalAppBinary() {
  if (!appBinary || !fs.existsSync(appBinary)) {
    throw new Error(
      "No local fbro binary found. Build one first, or set FBRO_APP_BINARY to the fbro executable.",
    );
  }
}

function ensureArtifactDirs() {
  fs.mkdirSync(path.join(artifactsDir, "screenshots"), { recursive: true });
  fs.mkdirSync(path.join(artifactsDir, "transcripts"), { recursive: true });
  fs.mkdirSync(path.join(artifactsDir, "logs"), { recursive: true });
}

function requireExecutable(name) {
  try {
    execFileSync("sh", ["-lc", `command -v ${name}`], { stdio: "ignore" });
  } catch {
    throw new Error(`Required executable not found in PATH: ${name}`);
  }
}

function requireDockerUsable() {
  requireExecutable("docker");
  try {
    execFileSync("docker", ["version", "--format", "{{.Server.Version}}"], { stdio: "ignore" });
  } catch {
    throw new Error("Docker is installed, but this user cannot access the Docker daemon");
  }
}

function writeLaunchWrapper(name, wsUrl) {
  ensureLocalAppBinary();
  const binDir = path.join(artifactsDir, "bin");
  fs.mkdirSync(binDir, { recursive: true });
  const wrapper = path.join(binDir, `${name}.sh`);
  fs.writeFileSync(wrapper, [
    "#!/usr/bin/env bash",
    "set -euo pipefail",
    "export FBRO_FRONTEND_ONLY=1",
    `export FBRO_WS_URL=${JSON.stringify(wsUrl)}`,
    `exec ${JSON.stringify(appBinary)} "$@"`,
    "",
  ].join("\n"));
  fs.chmodSync(wrapper, 0o755);
  return wrapper;
}

function spawnLogged(command, args, name, options = {}) {
  const log = fs.openSync(path.join(artifactsDir, "logs", `${name}.log`), "a");
  const child = spawn(command, args, {
    cwd: repoRoot,
    env: process.env,
    stdio: ["ignore", log, log],
    ...options,
  });
  child.once("exit", (code, signal) => {
    if (code !== null && code !== 0) {
      console.error(`${name} exited with ${code}`);
    }
    if (signal) console.error(`${name} exited on ${signal}`);
  });
  child.once("error", (error) => {
    console.error(`${name} failed to start: ${error.message}`);
  });
  return child;
}

async function waitForWebSocket(url, label) {
  await waitFor(() => new Promise((resolve) => {
    const ws = new WebSocket(url);
    const timer = setTimeout(() => {
      ws.terminate();
      resolve(false);
    }, 800);
    ws.once("open", () => {
      clearTimeout(timer);
      ws.close();
      resolve(true);
    });
    ws.once("error", () => {
      clearTimeout(timer);
      resolve(false);
    });
  }), { timeout: 60_000, interval: 500, label });
}

async function startLocalBackends() {
  requireDockerUsable();
  execFileSync("docker", [
    "compose",
    "-f",
    "docker-compose.e2e.yml",
    "up",
    "-d",
    "--build",
    "fbro-a",
    "fbro-b",
  ], { cwd: repoRoot, stdio: "inherit" });
  await Promise.all([
    waitForWebSocket(wsUrls.a, "fbro-a backend websocket"),
    waitForWebSocket(wsUrls.b, "fbro-b backend websocket"),
  ]);
}

async function startLocalDrivers() {
  requireExecutable("tauri-driver");
  const drivers = [
    spawnLogged("tauri-driver", ["--port", String(webdriverPorts.a)], "tauri-driver-a"),
    spawnLogged("tauri-driver", ["--port", String(webdriverPorts.b)], "tauri-driver-b"),
  ];
  await Promise.all([
    waitForWebDriver("fbro-a", webdriverHosts.a, webdriverPorts.a),
    waitForWebDriver("fbro-b", webdriverHosts.b, webdriverPorts.b),
  ]);
  return drivers;
}

function stopProcesses(processes) {
  for (const child of processes) {
    if (!child.killed) child.kill("SIGTERM");
  }
}

class FbroObserver {
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

  waitForPanelReady(panelId, timeout = 15_000) {
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      return panel?.status === "ready" ? panel : false;
    }, { timeout, label: `${this.name} panel ${panelId} ready` });
  }

  waitForEntry(panelId, entryName, kind, timeout = 15_000) {
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      const entry = panel?.entries?.find((candidate) => candidate.name === entryName);
      if (!entry) return false;
      return kind && entry.kind !== kind ? false : entry;
    }, { timeout, label: `${this.name} ${panelId} entry ${entryName}` });
  }

  waitForNoEntry(panelId, entryName, timeout = 15_000) {
    return waitFor(() => {
      const panel = this.state.panels[panelId];
      return panel && !panel.entries?.some((candidate) => candidate.name === entryName);
    }, { timeout, label: `${this.name} ${panelId} no entry ${entryName}` });
  }

  waitForJobIdle(timeout = 30_000) {
    return waitFor(() => {
      const active = Object.values(this.state.jobs).filter((job) =>
        job.state === "Queued" || job.state === "Running" || job.state === "AwaitingDecision"
      );
      return active.length === 0;
    }, { timeout, label: `${this.name} jobs idle` });
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

async function waitForWebDriver(name, host, port) {
  await waitFor(async () => {
    try {
      const response = await fetch(`http://${host}:${port}/status`);
      return response.ok;
    } catch {
      return false;
    }
  }, {
    timeout: 60_000,
    interval: 500,
    label: `${name} WebDriver status`,
  });
}

async function startBrowser(name, host, port, application) {
  await waitForWebDriver(name, host, port);
  const browser = await remote({
    protocol: "http",
    hostname: host,
    port,
    path: "/",
    logLevel: "warn",
    capabilities: {
      browserName: "wry",
      "tauri:options": { application },
    },
  });
  await browser.setWindowSize(1280, 800);
  await waitFor(() => browser.$('[data-testid="panel-left"]').then((el) => el.isExisting()), {
    timeout: 30_000,
    label: `${name} main window`,
  });
  return browser;
}

async function clickCommand(browser, commandId) {
  const el = await browser.$(`[data-command-id="${cssString(commandId)}"]`);
  await el.waitForClickable({ timeout: 10_000 });
  await el.click();
}

async function clickEntry(browser, panelId, name) {
  const el = await browser.$(`[data-testid="panel-${panelId}"] [data-testid="panel-entry"][data-entry-name="${cssString(name)}"]`);
  await el.waitForClickable({ timeout: 10_000 });
  await el.click();
  return el;
}

async function doubleClickEntry(browser, panelId, name) {
  const el = await clickEntry(browser, panelId, name);
  await el.doubleClick();
}

async function pressPanelKey(browser, panelId, key, modifiers = {}) {
  await browser.execute(({ panelId, key, modifiers }) => {
    const el = document.querySelector(`[data-testid="panel-${panelId}"]`);
    el?.focus();
    el?.dispatchEvent(new KeyboardEvent("keydown", {
      key,
      code: key.startsWith("F") ? key : undefined,
      shiftKey: !!modifiers.shift,
      ctrlKey: !!modifiers.ctrl,
      altKey: !!modifiers.alt,
      metaKey: !!modifiers.meta,
      bubbles: true,
      cancelable: true,
    }));
  }, { panelId, key, modifiers });
}

async function createDirectory(browser, observer, panelId, name) {
  await browser.execute((panelId) => document.querySelector(`[data-testid="panel-${panelId}"]`)?.focus(), panelId);
  await clickCommand(browser, "file.mkdir");
  const input = await browser.$('[data-testid="mkdir-name-input"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await input.setValue(name);
  await clickCommand(browser, "file.mkdir.confirm");
  await observer.waitForEntry(panelId, name, "directory");
}

async function createFile(browser, observer, panelId, name) {
  await pressPanelKey(browser, panelId, "F4", { shift: true });
  const input = await browser.$('[data-testid="touch-name-input"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await input.setValue(name);
  await clickCommand(browser, "file.touch.confirm");
  await observer.waitForEntry(panelId, name, "file");
}

async function confirmOperation(browser, commandId) {
  const dialog = await browser.$('[data-testid="operation-dialog"]');
  await dialog.waitForDisplayed({ timeout: 10_000 });
  await clickCommand(browser, commandId);
}

async function closeTerminalJobDialog(browser) {
  const close = await waitFor(async () => {
    const el = await browser.$('[data-command-id="job.close"]');
    return await el.isExisting() ? el : false;
  }, { timeout: 5_000, interval: 100, label: "terminal job close button" }).catch(() => null);
  if (close) {
    await close.click();
  }
}

function writeFinalTree() {
  const sections = [];
  for (const instance of ["a", "b"]) {
    const root = hostRoots[instance];
    try {
      sections.push(`## ${backendRoots[instance]} (${root})\n${execFileSync("find", [root, "-maxdepth", "5", "-print"]).toString()}`);
    } catch (error) {
      sections.push(`## ${backendRoots[instance]} (${root})\n${error instanceof Error ? error.message : String(error)}`);
    }
  }
  fs.writeFileSync(path.join(artifactsDir, "final-tree.txt"), sections.join("\n"));
}

async function runLocalFlow(browser, observer) {
  const aSrc = backendPath("a", "src");
  const aDst = backendPath("a", "dst");
  await createDirectory(browser, observer, "left", "alpha");
  await doubleClickEntry(browser, "left", "alpha");
  await waitFor(() => observer.state.panels.left?.path === path.posix.join(aSrc, "alpha"), {
    label: "left navigated into alpha",
  });
  await createDirectory(browser, observer, "left", "nested");
  await createFile(browser, observer, "left", "note.txt");
  await createFile(browser, observer, "left", "remove.txt");

  fs.writeFileSync(hostPath(path.posix.join(aSrc, "alpha/note.txt")), "renamed payload\n");
  fs.writeFileSync(hostPath(path.posix.join(aSrc, "alpha/remove.txt")), "delete me\n");

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc, cursorHint: "alpha" });
  await waitFor(() => observer.state.panels.left?.path === aSrc, { label: "left back to source" });
  await observer.waitForEntry("left", "alpha", "directory");
  await clickEntry(browser, "left", "alpha");
  await clickCommand(browser, "file.copy");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await observer.waitForEntry("right", "alpha", "directory");
  await closeTerminalJobDialog(browser);

  await doubleClickEntry(browser, "right", "alpha");
  await waitFor(() => observer.state.panels.right?.path === path.posix.join(aDst, "alpha"), {
    label: "right navigated into copied alpha",
  });
  await clickEntry(browser, "right", "note.txt");
  await clickCommand(browser, "file.rename");
  const renameInput = await browser.$('[data-testid="rename-to-input"]');
  await renameInput.waitForDisplayed({ timeout: 10_000 });
  await renameInput.setValue("renamed.txt");
  await clickCommand(browser, "file.rename.confirm");
  await observer.waitForEntry("right", "renamed.txt", "file");

  await clickEntry(browser, "right", "remove.txt");
  await clickCommand(browser, "file.delete");
  await confirmOperation(browser, "file.delete.confirm");
  await observer.waitForJobIdle();
  await observer.waitForNoEntry("right", "remove.txt");
  await closeTerminalJobDialog(browser);

  const copied = fs.readFileSync(hostPath(path.posix.join(aDst, "alpha/renamed.txt")), "utf8");
  if (copied !== "renamed payload\n") throw new Error("renamed copied file content mismatch");
  if (fs.existsSync(hostPath(path.posix.join(aDst, "alpha/remove.txt")))) throw new Error("deleted file still exists");
}

async function runArchiveFlow(browser, observer) {
  const aSrc = backendPath("a", "src");
  const aDst = backendPath("a", "dst");
  const root = path.posix.join(aSrc, "archive-root/nested");
  fs.mkdirSync(hostPath(root), { recursive: true });
  fs.writeFileSync(hostPath(path.posix.join(root, "payload.txt")), "archive payload\n");
  execFileSync("zip", ["-qr", "fixture.zip", "archive-root"], { cwd: hostPath(aSrc) });
  fs.rmSync(hostPath(path.posix.join(aSrc, "archive-root")), { recursive: true, force: true });

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc, cursorHint: "fixture.zip" });
  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "right", path: aDst });
  await observer.waitForEntry("left", "fixture.zip", "file");
  await clickEntry(browser, "left", "fixture.zip");
  await pressPanelKey(browser, "left", "Enter");
  await waitFor(() => observer.state.panels.left?.shadowSource, { timeout: 20_000, label: "zip mounted" });
  await observer.waitForEntry("left", "archive-root", "directory");
  await doubleClickEntry(browser, "left", "archive-root");
  await observer.waitForEntry("left", "nested", "directory");
  await doubleClickEntry(browser, "left", "nested");
  await observer.waitForEntry("left", "payload.txt", "file");
  await clickEntry(browser, "left", "payload.txt");
  await clickCommand(browser, "file.copy");
  await confirmOperation(browser, "file.copy.confirm");
  await observer.waitForJobIdle();
  await closeTerminalJobDialog(browser);

  const extracted = fs.readFileSync(hostPath(path.posix.join(aDst, "payload.txt")), "utf8");
  if (extracted !== "archive payload\n") throw new Error("archive extraction content mismatch");
}

async function runSearchFlow(browser, observer) {
  const aDst = backendPath("a", "dst");
  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aDst });
  await waitFor(() => observer.state.panels.left?.path === aDst, { label: "left at destination for search" });
  await browser.execute(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "f",
      code: "KeyF",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    }));
  });
  const input = await browser.$('[data-testid="search-file-mask-input"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await input.setValue("renamed.txt");
  await clickCommand(browser, "search.start");
  await waitFor(() => Object.values(observer.state.jobs).some((job) => job.kind === "search" && job.searchMatched > 0), {
    timeout: 20_000,
    label: "search match",
  });
  await clickCommand(browser, "search.to-results");
  await waitFor(() => observer.state.panels.left?.sourceId?.startsWith("search:"), {
    timeout: 10_000,
    label: "search results mounted",
  });
  await observer.waitForEntry("left", "renamed.txt", "file");
}

async function runRemoteFlow(browserA, observerA, browserB, observerB) {
  const aSrc = backendPath("a", "src");
  const bDst = backendPath("b", "dst");
  fs.writeFileSync(hostPath(path.posix.join(aSrc, "remote.txt")), "remote payload\n");
  observerA.sendIntent("Refresh", "left");
  await observerA.waitForEntry("left", "remote.txt", "file");

  const sharedSourceIds = Object.values(observerA.state.sources)
    .filter((source) => source.kind === "filesystem" && !source.shadow)
    .map((source) => source.id);
  if (sharedSourceIds.length === 0) throw new Error("no filesystem source to share");

  observerA.sendIntent("SetSharedSources", undefined, { sourceIds: sharedSourceIds });
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
  const remoteSource = Object.values(observerB.state.sources).find((source) => source.kind === "remote");
  if (!remoteSource.capabilities?.readOnly) throw new Error("remote source is not marked read-only");
  if (remoteSource.supports.includes("DeleteSelection") || remoteSource.supports.includes("NewFolder")) {
    throw new Error("remote source exposes write capabilities");
  }

  observerB.sendIntent("NavigateToSource", "left", { sourceId: remoteSource.id });
  await waitFor(() => observerB.state.panels.left?.sourceId === remoteSource.id, {
    timeout: 15_000,
    label: "b left uses remote source",
  });
  observerB.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: aSrc, cursorHint: "remote.txt" });
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
  const copied = fs.readFileSync(hostPath(path.posix.join(bDst, "remote.txt")), "utf8");
  if (copied !== "remote payload\n") throw new Error("remote copied file content mismatch");

  await browserA.getTitle();
  await browserB.getTitle();
}

async function main() {
  let browserA;
  let browserB;
  let observerA;
  let observerB;
  let localDrivers = [];
  let artifactsReady = false;
  try {
    if (localMode) {
      requireDockerUsable();
      requireExecutable("tauri-driver");
      requireExecutable("WebKitWebDriver");
      ensureLocalAppBinary();
      ensureArtifactDirs();
      artifactsReady = true;
      await startLocalBackends();
      localDrivers = await startLocalDrivers();
    } else {
      ensureArtifactDirs();
      artifactsReady = true;
    }

    const appA = localMode ? writeLaunchWrapper("fbro-a", wsUrls.a) : appBinary;
    const appB = localMode ? writeLaunchWrapper("fbro-b", wsUrls.b) : appBinary;

    browserA = await startBrowser("fbro-a", webdriverHosts.a, webdriverPorts.a, appA);
    observerA = new FbroObserver("fbro-a", wsUrls.a);
    await observerA.connect();

    await runLocalFlow(browserA, observerA);
    await runArchiveFlow(browserA, observerA);
    await runSearchFlow(browserA, observerA);

    browserB = await startBrowser("fbro-b", webdriverHosts.b, webdriverPorts.b, appB);
    observerB = new FbroObserver("fbro-b", wsUrls.b);
    await observerB.connect();
    await runRemoteFlow(browserA, observerA, browserB, observerB);

  } catch (error) {
    if (browserA) await browserA.saveScreenshot(path.join(artifactsDir, "screenshots", "fbro-a-failure.png")).catch(() => {});
    if (browserB) await browserB.saveScreenshot(path.join(artifactsDir, "screenshots", "fbro-b-failure.png")).catch(() => {});
    throw error;
  } finally {
    if (artifactsReady) {
      writeFinalTree();
      observerA?.close();
      observerB?.close();
    }
    if (browserA) await browserA.deleteSession().catch(() => {});
    if (browserB) await browserB.deleteSession().catch(() => {});
    stopProcesses(localDrivers);
  }
}

await main();
