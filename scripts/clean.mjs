import { spawnSync } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

export const jsOutputDirs = ["dist", path.join("node_modules", ".vite")];

export const cargoManifests = [
  path.join("src-tauri", "Cargo.toml"),
  path.join("tools", "lfs-spy", "Cargo.toml"),
];

function removeTree(target) {
  rmSync(target, { recursive: true, force: true });
}

function runCargo(command, args, options) {
  const result = spawnSync(command, args, options);
  if (result.error) return { status: 1, error: result.error };
  return { status: result.status ?? 1 };
}

export function cleanProject(rootDir, io = {}) {
  const remove = io.remove ?? removeTree;
  const run = io.run ?? runCargo;

  for (const relative of jsOutputDirs) {
    remove(path.join(rootDir, relative));
  }

  const failed = [];
  for (const relative of cargoManifests) {
    const manifest = path.join(rootDir, relative);
    const result = run("cargo", ["clean", "--manifest-path", manifest], {
      cwd: rootDir,
      stdio: "inherit",
    });
    if (!result || result.status !== 0) {
      const detail = result?.error ? `: ${result.error.message}` : "";
      failed.push(`${relative}${detail}`);
    }
  }

  if (failed.length > 0) {
    throw new Error(`cargo clean failed for ${failed.join(", ")}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    cleanProject(process.cwd());
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
