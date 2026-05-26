import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const smokeDir = path.dirname(fileURLToPath(import.meta.url));
export const repoRoot = path.resolve(smokeDir, "../../..");
export const localMode = process.env.FBRO_E2E_LOCAL === "1";
export const keepDocker = process.env.FBRO_E2E_KEEP_DOCKER === "1";
export const artifactsDir = process.env.FBRO_E2E_ARTIFACTS ?? path.resolve("artifacts/e2e");
export const composeFiles = (process.env.FBRO_E2E_COMPOSE_FILES ?? "docker-compose.e2e.yml")
  .split(path.delimiter)
  .filter(Boolean);

export const appBinary = process.env.FBRO_APP_BINARY ?? (
  localMode
    ? ["src-tauri/target/release/fbro", "src-tauri/target/debug/fbro"]
        .map((candidate) => path.join(repoRoot, candidate))
        .find((candidate) => fs.existsSync(candidate))
    : "/app/src-tauri/target/release/fbro"
);

export const backendRoots = { a: "/work/a", b: "/work/b" };

export const hostRoots = {
  a: process.env.FBRO_A_HOST_ROOT ?? (localMode ? path.join(artifactsDir, "fbro-a/work") : backendRoots.a),
  b: process.env.FBRO_B_HOST_ROOT ?? (localMode ? path.join(artifactsDir, "fbro-b/work") : backendRoots.b),
};

export const wsUrls = {
  a: process.env.FBRO_A_WS_URL ?? (localMode ? "ws://127.0.0.1:17878" : "ws://fbro-a:7878"),
  b: process.env.FBRO_B_WS_URL ?? (localMode ? "ws://127.0.0.1:27878" : "ws://fbro-b:7878"),
};

export const webdriverHosts = {
  a: process.env.FBRO_A_WEBDRIVER_HOST ?? (localMode ? "127.0.0.1" : "fbro-a"),
  b: process.env.FBRO_B_WEBDRIVER_HOST ?? (localMode ? "127.0.0.1" : "fbro-b"),
};

export const webdriverPorts = {
  a: Number(process.env.FBRO_A_WEBDRIVER_PORT ?? 4444),
  b: Number(process.env.FBRO_B_WEBDRIVER_PORT ?? (localMode ? 4445 : 4444)),
};

export const nativeWebdriverPorts = {
  a: Number(process.env.FBRO_A_NATIVE_WEBDRIVER_PORT ?? 4446),
  b: Number(process.env.FBRO_B_NATIVE_WEBDRIVER_PORT ?? 4447),
};

export function backendPath(instance, ...parts) {
  return path.posix.join(backendRoots[instance], ...parts);
}

export function hostPath(backendPathValue) {
  for (const [instance, backendRoot] of Object.entries(backendRoots)) {
    if (backendPathValue === backendRoot || backendPathValue.startsWith(`${backendRoot}/`)) {
      return path.join(hostRoots[instance], backendPathValue.slice(backendRoot.length));
    }
  }
  return backendPathValue;
}
