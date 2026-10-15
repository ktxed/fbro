import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import { captureGeometry, ffmpegArgs, repoRoot, run, sleep, toolDir, until } from "./core.mjs";

export async function buildNative() {
  if (process.platform !== "darwin") throw new Error("Native showcase recording requires macOS; list and storyboard also work elsewhere");
  const binary = path.join(repoRoot, "artifacts/showcase/bin/macos");
  const source = path.join(toolDir, "macos.swift");
  if (!fs.existsSync(binary) || fs.statSync(binary).mtimeMs < fs.statSync(source).mtimeMs) {
    fs.mkdirSync(path.dirname(binary), { recursive: true });
    await run("swiftc", [source, "-module-cache-path", path.join(process.env.TMPDIR ?? "/tmp", "fbro-showcase-swift-cache"), "-o", binary], { timeout: 120000 });
  }
  return binary;
}

export async function nativeCall(binary, ...args) {
  const { stdout } = await run(binary, args.map(String), { maxBuffer: 24 * 1024 * 1024, timeout: 15000 });
  return JSON.parse(stdout);
}

export async function prerequisites(config) {
  const native = await buildNative();
  const status = await nativeCall(native, "status");
  const checks = [];
  for (const [command, args] of [["cliclick", ["-V"]], ["ffmpeg", ["-version"]], ["ffprobe", ["-version"]], ["git", ["--version"]], ["nano", null]]) {
    try { const { stdout } = await run(args ? command : "/usr/bin/which", args ?? [command], { timeout: 5000 }); checks.push({ tool: command, available: true, version: stdout.split("\n")[0] }); }
    catch (error) { checks.push({ tool: command, available: false, error: error.code ?? error.message }); }
  }
  const app = fs.existsSync(config.app);
  const display = status.displays[config.screenIndex];
  const ready = app && checks.every((check) => check.available) && status.accessibility && status.screenRecording && display?.width > 0;
  return { ready: Boolean(ready), app, native, ...status, checks };
}

export function matchesNode(node, target) {
  if (target.role && !(new RegExp(target.role)).test(node.role)) return false;
  if (target.focused && !node.focused) return false;
  if (!node.enabled || !node.frame || node.frame.width <= 0 || node.frame.height <= 0) return false;
  const values = [node.title, node.description, node.help, node.value, node.identifier].filter(Boolean);
  if (target.label === undefined) return true;
  return values.some((text) => target.prefix ? text.startsWith(target.label) : text === target.label);
}

function entryTargets(nodes, name) {
  const targets = [];
  for (let index = 0; index < nodes.length; index++) {
    const row = nodes[index];
    if (row.role !== "AXRow") continue;
    const text = [];
    for (let child = index + 1; child < nodes.length && nodes[child].depth > row.depth; child++) {
      const node = nodes[child];
      if (node.depth === row.depth + 1 && node.role === "AXStaticText") text.push(node);
    }
    const base = text[0]?.value;
    const extension = text[1]?.value?.startsWith(".") ? text[1].value : "";
    if (base !== name && `${base}${extension}` !== name) continue;
    // Click the filename text, avoiding inline action buttons elsewhere in
    // the row. fbro exposes its basename and extension as separate AX nodes.
    targets.push({ ...row, frame: text[0].frame });
  }
  return targets;
}

export class MacInput {
  constructor(binary, pid, config, observer) {
    Object.assign(this, { binary, pid, config, observer });
  }

  trace(kind, details) {
    if (this.logFile) fs.appendFileSync(this.logFile, `${JSON.stringify({ at: new Date().toISOString(), kind, ...details })}\n`);
  }

  async inspect() {
    this.observer?.check();
    const state = await nativeCall(this.binary, "inspect", this.pid);
    if (!state.frontmost) throw new Error("fbro lost foreground focus; input stopped");
    if (this.stage) {
      const main = state.windows.find((window) => window.title === this.mainTitle);
      if (!main || Object.keys(this.stage).some((key) => Math.abs(main.frame[key] - this.stage[key]) > 1)) throw new Error("fbro window geometry changed; recalibrate before retrying");
    }
    return state;
  }

  async place() {
    await nativeCall(this.binary, "activate", this.pid);
    const status = await nativeCall(this.binary, "status");
    this.display = status.displays[this.config.screenIndex];
    if (!this.display) throw new Error("Configured display is unavailable");
    const width = Math.floor(Math.min(this.config.windowWidth, this.display.width - 80, (this.display.height - 100) * 16 / 9) / 16) * 16;
    if (width < 800) throw new Error("Display is too small for readable 16:9 footage");
    const height = width * 9 / 16;
    await until("fbro native window", async () => (await nativeCall(this.binary, "inspect", this.pid)).windows.length, this.config.timeoutMs);
    const desired = this.config.captureWorkingArea ? this.display.workingArea
      : { x: this.display.x + (this.display.width - width) / 2, y: this.display.y + 50, width, height };
    let state;
    await until("complete recording stage", async () => {
      await nativeCall(this.binary, "place", this.pid, desired.x, desired.y, desired.width, desired.height);
      await sleep(500);
      state = await nativeCall(this.binary, "inspect", this.pid);
      const actual = state.windows[0].frame;
      return Object.keys(desired).every(key => Math.abs(actual[key] - desired[key]) <= 1);
    }, this.config.timeoutMs);
    this.stage = state.windows[0].frame;
    this.mainTitle = state.windows[0].title;
    this.geometry = captureGeometry(this.stage, this.display);
    return this.stage;
  }

  async contain() {
    await nativeCall(this.binary, "contain", this.pid, ...["x", "y", "width", "height"].map((key) => this.stage[key]));
  }

  async find(target, { optional = false, panel } = {}) {
    let result;
    await until(`accessible ${JSON.stringify(target)}`, async () => {
      const state = await this.inspect();
      const candidates = target.entryName ? entryTargets(state.nodes, target.entryName) : state.nodes;
      const nodes = candidates.filter((node) => node.window === state.focusedWindow && matchesNode(node, target)
        && (!panel || (panel === "left" ? node.frame.x < this.stage.x + this.stage.width / 2 : node.frame.x >= this.stage.x + this.stage.width / 2))
        && node.frame.x >= this.stage.x && node.frame.y >= this.stage.y
        && node.frame.x + node.frame.width <= this.stage.x + this.stage.width + 1
        && node.frame.y + node.frame.height <= this.stage.y + this.stage.height + 1);
      // Prefer the smallest exact target, avoiding large ancestor groups.
      nodes.sort((a, b) => a.frame.width * a.frame.height - b.frame.width * b.frame.height);
      if (nodes.length) { result = nodes[0]; return true; }
      if (optional) return true;
      return false;
    }, this.config.timeoutMs);
    return result;
  }

  async events(args) {
    await this.inspect();
    await run("cliclick", args, { timeout: 15000 });
  }

  async move(x, y, click = false) {
    if (!Number.isFinite(x) || !Number.isFinite(y) || x < this.stage.x || y < this.stage.y || x >= this.stage.x + this.stage.width || y >= this.stage.y + this.stage.height) throw new Error("Pointer target is outside the filming stage");
    const coordinate = (n) => n < 0 ? `=${Math.round(n)}` : String(Math.round(n));
    await this.events(["-e", "8", `m:${coordinate(x)},${coordinate(y)}`, ...(click ? ["w:120", "c:."] : [])]);
  }

  async click(target, options = {}) {
    const node = await this.find(target, { ...options, optional: Boolean(this.config.anchors[options.anchor]) });
    if (node) return this.move(node.frame.x + node.frame.width / 2, node.frame.y + node.frame.height / 2, true);
    const anchor = this.config.anchors[options.anchor];
    if (!anchor || anchor.windowWidth !== this.stage.width || anchor.windowHeight !== this.stage.height) throw new Error(`No valid calibration for ${options.anchor}`);
    await this.move(this.stage.x + anchor.x * this.stage.width, this.stage.y + anchor.y * this.stage.height, true);
  }

  async button(label, options = {}) {
    return this.click({ label, role: "AXButton|AXRadioButton|AXCheckBox", prefix: options.prefix ?? false }, options);
  }

  async key(key, modifiers = []) {
    await this.inspect();
    this.trace("key", { key, modifiers });
    await nativeCall(this.binary, "key", this.pid, key, ...modifiers);
  }

  async type(text, replace = false) {
    this.trace("type", { text, replace });
    if (replace) await this.key("a", ["cmd"]);
    if (text === "") { if (replace) await this.key("Backspace"); return; }
    const lines = text.split("\n");
    for (let index = 0; index < lines.length; index++) {
      if (lines[index]) {
        await this.inspect();
        await nativeCall(this.binary, "text", this.pid, lines[index]);
      }
      if (index < lines.length - 1) await this.key("Enter");
    }
  }

  async command(id) {
    const { command, binding } = this.observer.binding(id);
    this.trace("command", { id, binding });
    if (!binding) {
      if (id === "app.command_palette") throw new Error("Command palette has no usable shortcut");
      await this.command("app.command_palette");
      await this.find({ role: "AXTextField|AXComboBox", focused: true });
      await this.type(command.title);
      // The combobox also exposes its query as static text. Include the
      // category so the click targets the actual command result beneath it.
      await this.click({ label: `${command.title} ${command.category}` }, { anchor: `command.${id}` });
      return;
    }
    const modifiers = [["ctrl", binding.ctrl], ["alt", binding.alt], ["cmd", binding.meta], ["shift", binding.shift]].filter(([, enabled]) => enabled).map(([key]) => key);
    await this.key(binding.key, modifiers);
  }

  async focusPanel(id) {
    await this.observer.refresh();
    if (this.observer.state.focus === id) return;
    await this.move(this.stage.x + this.stage.width * (id === "left" ? 0.23 : 0.73), this.stage.y + this.stage.height * 0.55, true);
    await this.observer.wait(`${id} panel focus`, (state) => state.focus === id);
  }

  async go(id, destination) {
    await this.focusPanel(id);
    await this.command("panel.navigate_to_path");
    await this.find({ role: "AXTextField", focused: true });
    await this.type(destination, true);
    await this.key("Enter");
    await this.observer.path(id, destination);
    await sleep(200);
  }

  async entry(id, name) {
    await this.focusPanel(id);
    const panel = this.observer.panel(id);
    if (panel.viewMode !== "list" || panel.filter?.value) throw new Error("Named-entry navigation requires an unfiltered list");
    const index = panel.entries.findIndex((entry) => entry.name === name);
    if (index < 0) throw new Error(`Entry is not in the backend listing: ${name}`);
    // Real navigation scrolls virtualized rows into view without changing the
    // filter or dropping selections that are temporarily hidden by filtering.
    let delta = index - panel.cursor;
    if (index < Math.abs(delta)) {
      await this.command("panel.cursor.first");
      await this.observer.wait("first listing row", () => this.observer.panel(id).cursor === 0);
      delta = index;
    }
    for (let step = 0; step < Math.abs(delta); step++) await this.command(delta >= 0 ? "panel.cursor.down" : "panel.cursor.up");
    await this.observer.wait(`cursor ${name}`, () => { const p = this.observer.panel(id); return p.entries[p.cursor]?.name === name; });
    const target = await this.find({ entryName: name }, { panel: id });
    // Navigation already put the real cursor on the file. Point at its name
    // without another click while macOS is settling scrolled AX row bounds.
    await this.move(target.frame.x + target.frame.width / 2, target.frame.y + target.frame.height / 2);
    await this.observer.wait(`cursor ${name}`, () => { const p = this.observer.panel(id); return p.entries[p.cursor]?.name === name; });
  }
}

export class Recorder {
  constructor(config, geometry, output, logFile) { Object.assign(this, { config, geometry, output, logFile }); }

  async start() {
    const log = fs.openSync(this.logFile, "wx");
    this.child = spawn("ffmpeg", ffmpegArgs(this.config, this.geometry, this.output), { stdio: ["pipe", "ignore", log] });
    fs.closeSync(log);
    this.exit = new Promise((resolve) => {
      this.child.once("error", (error) => resolve({ error }));
      this.child.once("exit", (code, signal) => resolve({ code, signal }));
    });
    this.child.stdin.on("error", () => {});
    await until("FFmpeg capture stream", async () => {
      if (this.child.exitCode !== null) throw new Error(`FFmpeg stopped; inspect ${this.logFile}`);
      const text = fs.readFileSync(this.logFile, "utf8");
      return /Output #0/.test(text);
    }, 15000);
    this.startedAt = performance.now();
  }

  async stop() {
    if (!this.child) return;
    if (this.child.exitCode === null) this.child.stdin.end("q\n");
    let timer;
    const result = await Promise.race([this.exit, new Promise((resolve) => { timer = setTimeout(() => { this.child.kill("SIGTERM"); resolve({ error: new Error("FFmpeg did not finalize within 15 seconds") }); }, 15000); })]);
    clearTimeout(timer);
    if (result.error || result.code !== 0) throw result.error ?? new Error(`FFmpeg exited ${result.code}; inspect ${this.logFile}`);
    const { stdout } = await run("ffprobe", ["-v", "error", "-show_streams", "-show_format", "-of", "json", this.output]);
    const info = JSON.parse(stdout);
    const video = info.streams.find((stream) => stream.codec_type === "video");
    if (!video || video.codec_name !== "h264" || video.width !== 1920 || video.height !== 1080 || Number(video.avg_frame_rate.split("/")[0]) / Number(video.avg_frame_rate.split("/")[1]) !== 30
        || info.streams.some((stream) => stream.codec_type === "audio") || Number(info.format.duration) <= 0) throw new Error(`Invalid showcase recording: ${this.output}`);
    return { durationSeconds: Number(info.format.duration), codec: video.codec_name, width: video.width, height: video.height, fps: 30, audio: false };
  }
}
