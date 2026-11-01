import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { cargoManifests, cleanProject, jsOutputDirs } from "../scripts/clean.mjs";

test("removes frontend build output and cargo-cleans both manifests", () => {
  const rootDir = mkdtempSync(path.join(os.tmpdir(), "fbro-clean-"));
  const removed = [];
  const cargo = [];
  try {
    mkdirSync(path.join(rootDir, "dist", "assets"), { recursive: true });
    mkdirSync(path.join(rootDir, "node_modules", ".vite"), { recursive: true });
    mkdirSync(path.join(rootDir, "node_modules", "left"), { recursive: true });
    writeFileSync(path.join(rootDir, "dist", "index.html"), "<html></html>\n");
    writeFileSync(path.join(rootDir, "node_modules", ".vite", "deps.json"), "{}\n");
    writeFileSync(path.join(rootDir, "node_modules", "left", "keep.txt"), "keep\n");
    writeFileSync(path.join(rootDir, "src.txt"), "stay\n");

    cleanProject(rootDir, {
      remove: (target) => {
        removed.push(path.relative(rootDir, target));
        rmSync(target, { recursive: true, force: true });
      },
      run: (command, args, options) => {
        cargo.push({ command, args, cwd: options.cwd, stdio: options.stdio });
        return { status: 0 };
      },
    });

    assert.deepEqual(removed, jsOutputDirs);
    assert.equal(existsSync(path.join(rootDir, "dist")), false);
    assert.equal(existsSync(path.join(rootDir, "node_modules", ".vite")), false);
    assert.equal(existsSync(path.join(rootDir, "node_modules", "left", "keep.txt")), true);
    assert.equal(existsSync(path.join(rootDir, "src.txt")), true);
    assert.deepEqual(
      cargo.map((call) => call.args),
      cargoManifests.map((manifest) => ["clean", "--manifest-path", path.join(rootDir, manifest)]),
    );
    assert.deepEqual(cargo.map((call) => call.command), ["cargo", "cargo"]);
    assert.deepEqual(cargo.map((call) => call.cwd), [rootDir, rootDir]);
    assert.deepEqual(cargo.map((call) => call.stdio), ["inherit", "inherit"]);
  } finally {
    rmSync(rootDir, { recursive: true, force: true });
  }
});

test("still cleans every Cargo manifest when one clean fails", () => {
  const rootDir = mkdtempSync(path.join(os.tmpdir(), "fbro-clean-"));
  const seen = [];
  try {
    assert.throws(
      () => cleanProject(rootDir, {
        remove: () => {},
        run: (_command, args) => {
          seen.push(args[2]);
          return args[2].endsWith(`${path.sep}lfs-spy${path.sep}Cargo.toml`)
            ? { status: 1 }
            : { status: 0 };
        },
      }),
      /cargo clean failed for tools[/\\]lfs-spy[/\\]Cargo.toml$/,
    );
    assert.equal(seen.length, cargoManifests.length);
  } finally {
    rmSync(rootDir, { recursive: true, force: true });
  }
});
