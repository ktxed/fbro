import fs from "node:fs";
import WebSocket from "ws";
import { until } from "./core.mjs";

export class Observer {
  constructor(url, logFile, timeout = 20000) {
    this.url = url;
    this.timeout = timeout;
    this.state = { panels: {}, jobs: {}, commands: [] };
    this.log = fs.createWriteStream(logFile, { flags: "wx" });
    this.versions = new Map();
    this.failure = null;
    this.snapshotSerial = 0;
  }

  async connect() {
    await until("dedicated fbro WebSocket", async () => {
      const ws = new WebSocket(this.url);
      const connected = await new Promise((resolve) => {
        const timer = setTimeout(() => { ws.terminate(); resolve(false); }, 700);
        ws.once("error", () => { clearTimeout(timer); resolve(false); });
        ws.once("open", () => { clearTimeout(timer); resolve(true); });
      });
      if (!connected) return false;
      this.ws = ws;
      ws.on("message", (data) => {
        try { this.receive(JSON.parse(String(data))); }
        catch (error) { this.failure = error; }
      });
      ws.on("error", (error) => { this.failure = error; });
      ws.on("close", () => { if (!this.closing) this.failure = new Error("fbro observer disconnected"); });
      ws.send(JSON.stringify({ type: "Hello", protocolVersion: "0", client: { kind: "observer", name: "fbro-showcase", supports: ["jobs", "decisions"] } }));
      return true;
    }, this.timeout);
    await this.wait("initial panels and command registry", (state) => state.commands.length && state.panels.left?.status === "ready" && state.panels.right?.status === "ready");
  }

  receive(message) {
    this.log.write(`${JSON.stringify({ receivedAt: new Date().toISOString(), ...message })}\n`);
    if (message.type === "StateSnapshot") {
      this.state = { ...this.state, ...message.domains,
        jobs: { ...this.state.jobs, ...Object.fromEntries((message.domains.jobs ?? []).map((job) => [job.jobId, job])) } };
      this.snapshotSerial++;
      for (const [id, panel] of Object.entries(this.state.panels)) {
        if (Number.isFinite(panel.version)) this.versions.set(`panels:${id}`, panel.version);
      }
    }
    if (message.type === "StatePatch") {
      const key = `${message.domain}:${message.entityId ?? ""}`;
      // Panel versions are monotonic. Some existing singleton domains (focus,
      // for example) reuse version 1; preserve their wire arrival order.
      if (message.domain === "panels") {
        if (message.version <= (this.versions.get(key) ?? -1)) return;
        this.versions.set(key, message.version);
      }
      const patch = message.patch ?? {};
      if (message.domain === "panels" && message.entityId) {
        const current = this.state.panels[message.entityId] ?? {};
        this.state.panels[message.entityId] = { ...current, ...patch,
          entries: patch.windowEntries ?? (patch.appendEntries ? [...(current.entries ?? []), ...patch.appendEntries] : (patch.entries ?? current.entries)) };
      }
      if (message.domain === "focus") this.state.focus = patch.focus;
      if (message.domain === "commands") this.state.commands = patch.commands ?? this.state.commands;
      if (message.domain === "jobs" && patch.removeJobId) delete this.state.jobs[patch.removeJobId];
    }
    if (message.type === "JobStarted") this.state.jobs[message.job.jobId] = { ...message.job, state: message.job.initialState };
    if (message.type === "JobEvent") {
      const job = this.state.jobs[message.jobId] ??= {};
      if (["Completed", "Failed", "Cancelled", "AwaitingDecision"].includes(message.event)) job.state = message.event;
      if (message.event === "Progress") job.state = "Running";
      if (message.event === "Failed") this.failure = new Error(`fbro job failed: ${JSON.stringify(message.error)}`);
    }
    if (message.type === "Error") this.failure = new Error(`fbro ${message.code}: ${message.message}`);
    if (message.type === "GitActionFinished" && !message.success) this.failure = new Error(`Git action failed: ${message.message ?? message.error ?? message.action}`);
  }

  check() { if (this.failure) throw this.failure; }

  requestSnapshot() {
    this.check();
    this.ws.send(JSON.stringify({ type: "Intent", name: "RequestStateSnapshot", scope: null, args: {}, options: {} }));
  }

  async refresh() {
    const before = this.snapshotSerial;
    this.requestSnapshot();
    await until("fresh backend snapshot", () => { this.check(); return this.snapshotSerial > before; }, this.timeout);
  }

  async wait(label, predicate) {
    // GUI-requested cursor/path/filter patches are not all broadcast to
    // observers. Existing read-only snapshots provide authoritative state.
    await this.refresh();
    let requestedAt = Date.now();
    return until(label, () => {
      this.check();
      if (Date.now() - requestedAt >= 400) { this.requestSnapshot(); requestedAt = Date.now(); }
      return predicate(this.state);
    }, this.timeout);
  }

  panel(id = "left") {
    this.check();
    const panel = this.state.panels[id];
    if (!panel) throw new Error(`Missing authoritative ${id} panel`);
    return panel;
  }

  async path(id, expected) {
    return this.wait(`${id} panel at ${expected}`, () => this.panel(id).path === expected && this.panel(id).status === "ready");
  }

  selected(id) {
    const panel = this.panel(id);
    return panel.selection.map((index) => panel.entries[index]?.name).sort();
  }

  async selection(id, names) {
    return this.wait(`${id} selection ${names.join(", ")}`, () => JSON.stringify(this.selected(id)) === JSON.stringify([...names].sort()));
  }

  binding(id) {
    const command = this.state.commands.find((item) => item.id === id);
    if (!command) throw new Error(`Command is unavailable in this app build: ${id}`);
    const binding = command.bindings.find((item) => !item.modes?.length || item.modes.includes(this.panel(this.state.focus).viewMode));
    return { command, binding };
  }

  async close() {
    this.closing = true;
    if (this.ws) { this.ws.close(); this.ws.terminate(); }
    await new Promise((resolve) => this.log.end(resolve));
  }
}
