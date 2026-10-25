// Packaged-app capability check. Permission UI is opt-in; no alerts are sent.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

assert.equal(process.platform, "darwin", "This check requires a macOS desktop session");
const repoRoot = fileURLToPath(new URL("../../", import.meta.url));
const binary = process.env.FBRO_APP_BINARY ?? path.join(repoRoot,
  "src-tauri/target/debug/bundle/macos/fbro.app/Contents/MacOS/fbro");
assert(fs.existsSync(binary), "Build the .app first: npm run tauri -- build --debug --bundles app");
const bundle = path.resolve(binary, "../../..");
execFileSync("codesign", ["--verify", "--deep", "--strict", bundle]);
const signature = spawnSync("codesign", ["--display", "--verbose=2", bundle], { encoding: "utf8" });
assert.equal(signature.status, 0, signature.stderr);
assert.match(signature.stderr, /^Identifier=dev\.fbro\.app$/m,
  "The app bundle must be signed with its notification identity; the linker signature is insufficient");
assert(!signature.stderr.includes("Info.plist=not bound"), "The bundle's Info.plist must be signed");
const requestPermission = process.argv.includes("--request-permission");
const root = fs.mkdtempSync(path.join(os.tmpdir(), "fbro-macos-notifications-"));
const config = path.join(root, "fbro");
fs.mkdirSync(config);
fs.writeFileSync(path.join(config, "settings.json"), JSON.stringify({ general: {
  show_hidden_default: false, remember_window_state: false, os_notifications_enabled: requestPermission,
} }));
fs.writeFileSync(path.join(config, "session.toml"),
  `[panels.left]\npath = ${JSON.stringify(root)}\n[panels.right]\npath = ${JSON.stringify(root)}\n`);
const server = net.createServer();
await new Promise((resolve, reject) => {
  server.once("error", reject);
  server.listen(0, "127.0.0.1", resolve);
});
const port = server.address().port;
await new Promise((resolve) => server.close(resolve));
const logPath = path.join(root, "app.log");
const appLog = fs.openSync(logPath, "wx");
const child = spawn(binary, [], {
  cwd: repoRoot,
  env: { ...process.env, XDG_CONFIG_HOME: root,
    FBRO_WS_BIND: `127.0.0.1:${port}`, FBRO_WS_URL: `ws://127.0.0.1:${port}` },
  stdio: ["ignore", appLog, appLog],
});
fs.closeSync(appLog);
let spawnError;
child.once("error", (error) => { spawnError = error; });
const exited = new Promise((resolve) => child.once("exit", resolve));
let socket;
try {
  for (let attempt = 0; attempt < 60; attempt++) {
    if (spawnError) throw spawnError;
    assert.equal(child.exitCode, null, `App exited; see ${logPath}`);
    const candidate = new WebSocket(`ws://127.0.0.1:${port}`);
    const connected = await new Promise((resolve) => {
      candidate.once("open", () => resolve(true));
      candidate.once("error", () => resolve(false));
    });
    if (connected) { socket = candidate; break; }
    await sleep(250);
  }
  assert(socket, `Packaged app did not start its backend; see ${logPath}`);
  const snapshot = new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`No snapshot; see ${logPath}`)), 15_000);
    socket.on("message", (bytes) => {
      const message = JSON.parse(String(bytes));
      if (message.type === "StateSnapshot") { clearTimeout(timer); resolve(message); }
    });
  });
  socket.send(JSON.stringify({ type: "Hello", protocolVersion: "0",
    client: { kind: "observer", name: "macos-notifications-smoke", supports: [] } }));
  const state = await snapshot;
  assert.equal(state.domains.settings.general.supportsOsNotifications, true);
  assert.equal(state.domains.settings.general.supportsOsNotificationSettings, true);
  assert.equal(state.domains.settings.general.osNotificationsEnabled, requestPermission);
  if (requestPermission) {
    // A second instance can start behind an existing fbro window. Activate this
    // exact PID through AppKit so the foreground-only permission hook executes.
    execFileSync("osascript", ["-l", "JavaScript", "-e",
      `ObjC.import('AppKit'); $.NSRunningApplication.runningApplicationWithProcessIdentifier(${child.pid}).activateWithOptions(1);`]);
    console.log(`Test fbro PID ${child.pid} activated; respond to the macOS notification prompt if shown.`);
    const deadline = Date.now() + 120_000;
    let completed = false;
    while (Date.now() < deadline) {
      const log = fs.readFileSync(logPath, "utf8");
      assert(!log.includes("OS authorization failed:"), `Native authorization failed; see ${logPath}\n${log}`);
      if (/OS notification authorization granted|OS notifications denied;/.test(log)) {
        console.log(log.split("\n").filter((line) => line.startsWith("[notifications]")).join("\n"));
        completed = true;
        break;
      }
      await sleep(250);
    }
    assert(completed, `No native authorization response within two minutes; see ${logPath}`);
  } else {
    assert(!fs.readFileSync(logPath, "utf8").includes("Requesting macOS alert authorization"),
      "Disabled startup must not request authorization");
  }
  if (process.argv.includes("--open-settings")) {
    const requestId = crypto.randomUUID();
    const opened = new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`No settings reply; see ${logPath}`)), 10_000);
      socket.on("message", (bytes) => {
        const message = JSON.parse(String(bytes));
        if (message.requestId === requestId && (message.type === "OpenOsNotificationSettingsOk"
          || message.type === "OpenOsNotificationSettingsFailed")) {
          clearTimeout(timer);
          resolve(message);
        }
      });
    });
    socket.send(JSON.stringify({ type: "Intent", name: "OpenOsNotificationSettings", args: { requestId } }));
    const reply = await opened;
    assert.equal(reply.type, "OpenOsNotificationSettingsOk", JSON.stringify(reply));
    console.log("Native fbro notification settings launch acknowledged.");
  }
  console.log(`Packaged macOS notification ${requestPermission ? "authorization" : "capability and disabled startup"} passed. Log: ${logPath}`);
} finally {
  socket?.terminate();
  if (!spawnError && child.exitCode === null && child.signalCode === null) {
    child.kill("SIGTERM");
    const timeout = setTimeout(() => child.kill("SIGKILL"), 5000);
    await exited;
    clearTimeout(timeout);
  }
}
