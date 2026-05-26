import fs from "node:fs";
import path from "node:path";
import { execFileSync, spawn } from "node:child_process";
import WebSocket from "ws";
import {
  appBinary,
  artifactsDir,
  backendRoots,
  composeFiles,
  hostRoots,
  keepDocker,
  nativeWebdriverPorts,
  repoRoot,
  webdriverHosts,
  webdriverPorts,
  wsUrls,
} from "./config.mjs";
import { waitFor } from "./wait.mjs";
import { waitForWebDriver } from "./browser.mjs";

export function ensureLocalAppBinary() {
  if (!appBinary || !fs.existsSync(appBinary)) {
    throw new Error(
      "No local fbro binary found. Build one first, or set FBRO_APP_BINARY to the fbro executable.",
    );
  }
}

export function ensureArtifactDirs() {
  fs.mkdirSync(path.join(artifactsDir, "screenshots"), { recursive: true });
  fs.mkdirSync(path.join(artifactsDir, "transcripts"), { recursive: true });
  fs.mkdirSync(path.join(artifactsDir, "logs"), { recursive: true });
}

export function requireExecutable(name) {
  try {
    execFileSync("sh", ["-lc", `command -v ${name}`], { stdio: "ignore" });
  } catch {
    throw new Error(`Required executable not found in PATH: ${name}`);
  }
}

export function requireDockerUsable() {
  requireExecutable("docker");
  try {
    execFileSync("docker", ["version", "--format", "{{.Server.Version}}"], { stdio: "ignore" });
  } catch {
    throw new Error("Docker is installed, but this user cannot access the Docker daemon");
  }
}

function dockerComposeArgs(...args) {
  return [
    "compose",
    ...composeFiles.flatMap((file) => ["-f", file]),
    ...args,
  ];
}

export function writeLaunchWrapper(name, wsUrl) {
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

export async function startLocalBackends() {
  requireDockerUsable();
  execFileSync("docker", dockerComposeArgs(
    "up",
    "-d",
    "--build",
    "fbro-a",
    "fbro-b",
  ), { cwd: repoRoot, stdio: "inherit" });
  await Promise.all([
    waitForWebSocket(wsUrls.a, "fbro-a backend websocket"),
    waitForWebSocket(wsUrls.b, "fbro-b backend websocket"),
  ]);
}

export function stopLocalBackends() {
  if (keepDocker) return;
  try {
    execFileSync("docker", dockerComposeArgs(
      "down",
      "--remove-orphans",
    ), { cwd: repoRoot, stdio: "inherit" });
  } catch (error) {
    console.error(`docker compose cleanup failed: ${error.message}`);
  }
}

export async function startLocalDrivers() {
  requireExecutable("tauri-driver");
  const drivers = [
    spawnLogged(
      "tauri-driver",
      ["--port", String(webdriverPorts.a), "--native-port", String(nativeWebdriverPorts.a)],
      "tauri-driver-a",
    ),
    spawnLogged(
      "tauri-driver",
      ["--port", String(webdriverPorts.b), "--native-port", String(nativeWebdriverPorts.b)],
      "tauri-driver-b",
    ),
  ];
  await Promise.all([
    waitForWebDriver("fbro-a", webdriverHosts.a, webdriverPorts.a),
    waitForWebDriver("fbro-b", webdriverHosts.b, webdriverPorts.b),
  ]);
  return drivers;
}

export function stopProcesses(processes) {
  for (const child of processes) {
    if (!child.killed) child.kill("SIGTERM");
  }
}

export function writeFinalTree() {
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
