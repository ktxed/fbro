#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import net from "node:net";
import { randomInt } from "node:crypto";
import { spawn } from "node:child_process";
import { parseArgs } from "node:util";
import { isDeepStrictEqual } from "node:util";
import { fileURLToPath } from "node:url";
import { commitMessage, loadConfig, repoRoot, run, showcaseFontSize, sleep, sourceBaseline, verifyBaseline, writeIndex } from "./core.mjs";
import { mediaBaseline, prepareFixture } from "./fixtures.mjs";
import { buildNative, MacInput, nativeCall, prerequisites, Recorder } from "./macos.mjs";
import { Observer } from "./observer.mjs";
import { chapters, playScene, Scene, selectChapters, startingPaths } from "./scenarios.mjs";

const help = `fbro macOS showcase

  npm run showcase -- list
  npm run showcase -- storyboard
  npm run showcase -- preflight [--capture]
  npm run showcase -- rehearse --clip 01
  npm run showcase -- record --clip all
  npm run showcase -- record --clip 09,10
  npm run showcase -- inspect --pid PID
  npm run showcase -- calibrate --pid PID --name ANCHOR

Options: --config FILE, --app BINARY, --pace NUMBER, --clip ID[,ID] (default all)
--capture records a short native capture check during preflight.
Each rehearsal/recording uses fresh demo destinations. Git scene 10 makes a
local README-only commit in an independent clone, including in rehearsal mode.
No tooling commits or pushes are performed. Ctrl+C stops the run and saves its index.
`;

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function stopChild(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  const exited = new Promise((resolve) => child.once("exit", resolve));
  let timer;
  await Promise.race([exited, new Promise((resolve) => { timer = setTimeout(() => { child.kill("SIGKILL"); resolve(); }, 3000); })]);
  clearTimeout(timer);
}

function seedSession(directory, paths, fontSize) {
  const configRoot = path.join(directory, "config");
  const appConfig = path.join(configRoot, "fbro");
  fs.mkdirSync(appConfig, { recursive: true });
  fs.writeFileSync(path.join(appConfig, "session.toml"), `font_size = ${fontSize}\n\n[panels.left]\npath = ${JSON.stringify(paths.left)}\n[panels.right]\npath = ${JSON.stringify(paths.right)}\n`);
  fs.writeFileSync(path.join(appConfig, "settings.json"), JSON.stringify({ general: {
    show_hidden_default: false, remember_window_state: false,
    quick_terminal_editor: "nano", compare_differ_mode: "internal", color_scheme: "fbro-dark",
  } }));
  // Avoid personal Nano keybindings/backup settings in the Quick Edit scene.
  const nanoConfig = path.join(configRoot, "nano");
  fs.mkdirSync(nanoConfig);
  fs.writeFileSync(path.join(nanoConfig, "nanorc"), "");
  return configRoot;
}

async function launch(directory, paths, config, binary) {
  fs.mkdirSync(directory, { recursive: true });
  const fontSize = config.initialFontSize ?? showcaseFontSize;
  const configRoot = seedSession(directory, paths, fontSize);
  const port = await freePort();
  const appLog = fs.openSync(path.join(directory, "app.log"), "wx");
  const child = spawn(config.app, [], { cwd: repoRoot, env: {
    ...process.env, XDG_CONFIG_HOME: configRoot, FBRO_WS_BIND: `127.0.0.1:${port}`, FBRO_WS_URL: `ws://127.0.0.1:${port}`,
  }, stdio: ["ignore", appLog, appLog] });
  fs.closeSync(appLog);
  const observer = new Observer(`ws://127.0.0.1:${port}`, path.join(directory, "backend.jsonl"), config.timeoutMs);
  child.once("error", (error) => { observer.failure = error; });
  child.once("exit", (code, signal) => { if (!observer.closing) observer.failure = new Error(`Dedicated fbro exited (${signal ?? code})`); });
  try {
    await observer.connect();
    await observer.wait("showcase zoom", (state) => state.fontSize === fontSize);
    const input = new MacInput(binary, child.pid, config, observer);
    input.logFile = path.join(directory, "input.jsonl");
    await input.place();
    await observer.path("left", paths.left); await observer.path("right", paths.right);
    process.stdout.write(`  Dedicated fbro PID ${child.pid}; stage ${input.stage.width}×${input.stage.height}\n`);
    return { child, observer, input };
  } catch (error) {
    await observer.close(); await stopChild(child); throw error;
  }
}

async function appVersion(app) {
  try {
    const { stdout } = await run("/usr/libexec/PlistBuddy", ["-c", "Print :CFBundleShortVersionString", path.resolve(app, "../../Info.plist")]);
    return stdout.trim();
  } catch { return "unknown (standalone binary)"; }
}

async function showPreflight(config, capture) {
  const status = await prerequisites(config);
  const info = { ...status, media: [] };
  for (const name of [config.photos[0], config.gif, config.video]) {
    try {
      const { stdout } = await run("ffprobe", ["-v", "error", "-show_entries", "stream=codec_name,codec_type,width,height:format=duration", "-of", "json", path.join(config.pics, name)]);
      info.media.push({ name, ...JSON.parse(stdout) });
    } catch (error) { info.ready = false; info.media.push({ name, error: error.message }); }
  }
  process.stdout.write(`${JSON.stringify(info, null, 2)}\n`);
  const report = path.join(repoRoot, "artifacts/showcase/preflight.json");
  fs.mkdirSync(path.dirname(report), { recursive: true });
  fs.writeFileSync(report, `${JSON.stringify({ checkedAt: new Date().toISOString(), ...info }, null, 2)}\n`);
  if (!info.ready) {
    process.stderr.write("Native preflight is not ready. Use a logged-in macOS desktop and enable Accessibility and Screen & System Audio Recording for the app/terminal launching this command. Reopen it after granting access. No demo files were created.\n");
    process.exitCode = 1;
    return;
  }
  if (capture) await execute(config, [], "preflight", status.native);
}

async function execute(config, selected, mode, binary) {
  const adjectives = ["happy", "sleepy", "dancing", "wiggly", "jolly", "bouncy", "curious", "fuzzy", "fancy", "cheeky", "wobbly", "dizzy", "snazzy", "merry", "cosmic", "sparkly"];
  const nouns = ["parakeet", "wombat", "pancake", "otter", "turnip", "penguin", "waffle", "badger", "dumpling", "narwhal", "teapot", "capybara", "noodle", "puffin", "muffin", "platypus"];
  const root = path.join(repoRoot, "artifacts/showcase");
  fs.mkdirSync(root, { recursive: true });
  let runId;
  // Try each playful pair once, in random order. Never reuse an existing
  // recording directory or a matching media destination.
  const candidates = adjectives.flatMap(adjective => nouns.map(noun => `${adjective}-${noun}`));
  while (candidates.length) {
    const [candidate] = candidates.splice(randomInt(candidates.length), 1);
    if (fs.existsSync(path.join(config.pics, "fbro-demo", candidate))) continue;
    try { fs.mkdirSync(path.join(root, candidate)); runId = candidate; break; }
    catch (error) { if (error.code !== "EEXIST") throw error; }
  }
  if (!runId) throw new Error("All playful demo names are already in use; archive old runs before recording again.");
  const directory = path.join(repoRoot, "artifacts/showcase", runId);
  const before = await sourceBaseline();
  const media = mediaBaseline(config);
  const manifest = { schemaVersion: 1, runId, mode, sourceRevision: before.head, sourceBranch: before.branch,
    app: config.app, appVersion: await appVersion(config.app), config, capture: { width: 1920, height: 1080, fps: 30, audio: false, cursor: true },
    assets: { ...media }, demoCommitMessage: commitMessage, clips: [], status: "running", startedAt: new Date().toISOString() };
  writeIndex(manifest, directory);
  let active, interrupted = false;
  const interrupt = () => { interrupted = true; if (active) active.observer.failure = new Error("Run interrupted by user"); };
  process.on("SIGINT", interrupt); process.on("SIGTERM", interrupt);
  try {
    const requests = mode === "preflight" ? [{ id: "00-capture-check", title: "Native capture check", description: "Check the real cursor, crop, readability, and JPEG viewer window." }] : selected;
    for (const chapter of requests) {
      if (interrupted) throw new Error("Run interrupted by user");
      process.stdout.write(`${chapter.id}: ${chapter.title}\n`);
      const clip = { ...chapter, status: "preparing", file: mode === "rehearse" ? null : `${chapter.id}.mp4`, actions: [] };
      manifest.clips.push(clip); writeIndex(manifest, directory);
      let recorder, takeStarted;
      try {
        const fixture = await prepareFixture(mode === "preflight" ? "01-browse-filter" : chapter.id, { config, runId, directory, revision: before.head });
        const paths = startingPaths(fixture, config);
        const captureWorkingArea = true;
        const initialFontSize = chapter.id === "01-browse-filter" ? 13 : showcaseFontSize;
        active = await launch(path.join(directory, `${chapter.id}-logs`), paths, { ...config, captureWorkingArea, initialFontSize }, binary);
        if (interrupted) throw new Error("Run interrupted by user");
        clip.stage = active.input.stage; clip.crop = active.input.geometry; clip.paths = paths;
        clip.initialFontSize = initialFontSize;
        if (fixture.clone) clip.demoClone = fixture.clone;
        if (mode !== "rehearse") {
          recorder = new Recorder(config, active.input.geometry, path.join(directory, clip.file), path.join(directory, `${chapter.id}-logs`, "ffmpeg.log"));
          await recorder.start();
        }
        const scene = new Scene(active.input, active.observer, fixture, config, clip);
        scene.startedAt = recorder?.startedAt ?? performance.now();
        takeStarted = scene.startedAt;
        clip.status = "running";
        if (mode === "preflight") {
          await sleep(1500);
          await scene.step("Move the visible cursor over the listing", () => active.input.move(active.input.stage.x + active.input.stage.width * 0.3, active.input.stage.y + active.input.stage.height * 0.5));
          await scene.step("Open a JPEG in its separate viewer", async () => { await scene.viewer(config.photos[0]); await scene.imageReady(); }, 1500);
          await scene.closeViewer(); await sleep(1000);
        } else await playScene(scene);
        if (interrupted) throw new Error("Run interrupted by user");
        clip.durationSeconds = (performance.now() - scene.startedAt) / 1000;
        clip.status = "passed";
      } catch (error) {
        clip.status = "failed"; clip.error = error.message;
        if (active) {
          try { fs.writeFileSync(path.join(directory, `${chapter.id}-logs`, "failure-ax.json"), JSON.stringify(await nativeCall(binary, "inspect", active.child.pid), null, 2)); } catch { /* The process may already be gone. */ }
        }
        throw error;
      } finally {
        if (takeStarted) clip.durationSeconds ??= (performance.now() - takeStarted) / 1000;
        try { if (recorder) Object.assign(clip, await recorder.stop()); }
        catch (error) { clip.status = "failed"; clip.error = [clip.error, error.message].filter(Boolean).join("; "); manifest.error = clip.error; }
        if (active) { await active.observer.close(); await stopChild(active.child); active = null; }
        writeIndex(manifest, directory);
      }
      if (clip.status === "failed") throw new Error(clip.error);
    }
    manifest.status = "passed";
  } catch (error) {
    manifest.status = "failed"; manifest.error = error.message;
    process.exitCode = 1;
  } finally {
    process.off("SIGINT", interrupt); process.off("SIGTERM", interrupt);
    try {
      verifyBaseline(before, await sourceBaseline());
      if (!isDeepStrictEqual(media, mediaBaseline(config))) throw new Error("Original media changed during recording");
      manifest.originalsVerified = true;
    } catch (error) { manifest.status = "failed"; manifest.error = [manifest.error, error.message].filter(Boolean).join("; "); process.exitCode = 1; }
    manifest.finishedAt = new Date().toISOString(); writeIndex(manifest, directory);
    process.stdout.write(`\n${manifest.status}: ${path.join(directory, "clips.md")}\n`);
    if (manifest.error) process.stderr.write(`${manifest.error}\n`);
  }
}

export async function main(args = process.argv.slice(2)) {
  const { values, positionals } = parseArgs({ args, allowPositionals: true, options: {
    config: { type: "string" }, app: { type: "string" }, pace: { type: "string" }, clip: { type: "string", default: "all" },
    pid: { type: "string" }, name: { type: "string" }, capture: { type: "boolean" }, help: { type: "boolean", short: "h" },
  } });
  if (values.help || !positionals.length) { process.stdout.write(help); return; }
  if (positionals.length !== 1) throw new Error("Expected one command; use --help");
  const command = positionals[0];
  if (command === "list" || command === "storyboard") {
    for (const chapter of chapters) process.stdout.write(`${chapter.id}  ${chapter.title}\n${command === "storyboard" ? `  ${chapter.description}\n` : ""}`);
    return;
  }
  if (["inspect", "calibrate"].includes(command)) {
    if (!/^\d+$/.test(values.pid ?? "")) throw new Error("Specify the dedicated fbro --pid");
    const binary = await buildNative();
    const state = await nativeCall(binary, "inspect", values.pid);
    if (command === "inspect") process.stdout.write(`${JSON.stringify(state, null, 2)}\n`);
    else {
      if (!values.name) throw new Error("Specify --name for the anchor");
      const { stdout } = await run("cliclick", ["p:."]);
      const [x, y] = stdout.trim().split(",").map(Number);
      const stage = state.windows.find((window) => window.title === "fbro")?.frame;
      if (!stage || !Number.isFinite(x) || !Number.isFinite(y) || x < stage.x || y < stage.y || x >= stage.x + stage.width || y >= stage.y + stage.height) throw new Error("Place the pointer inside the main fbro window before calibration");
      process.stdout.write(`${JSON.stringify({ [values.name]: { x: (x - stage.x) / stage.width, y: (y - stage.y) / stage.height, windowWidth: stage.width, windowHeight: stage.height } }, null, 2)}\n`);
    }
    return;
  }
  if (!["preflight", "record", "rehearse"].includes(command)) throw new Error(`Unknown command: ${command}`);
  const config = loadConfig(values.config, { ...(values.app ? { app: values.app } : {}), ...(values.pace ? { pace: Number(values.pace) } : {}) });
  if (command === "preflight") return showPreflight(config, values.capture);
  const selected = selectChapters(values.clip);
  const status = await prerequisites(config);
  if (!status.ready) {
    process.stderr.write(`${JSON.stringify(status, null, 2)}\n`);
    throw new Error("Native capture/input is not ready. Run npm run showcase -- preflight and resolve the reported permissions/display/tools first; no demo files were created");
  }
  await execute(config, selected, command, status.native);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
