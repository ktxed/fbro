import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { collectBuildInfo } from "../scripts/build-info.mjs";

function git(rootDir, ...args) {
  return execFileSync("git", ["-C", rootDir, ...args], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function fixture({ repository = true } = {}) {
  const rootDir = mkdtempSync(path.join(os.tmpdir(), "fbro-build-info-"));
  mkdirSync(path.join(rootDir, "src-tauri"));
  writeFileSync(path.join(rootDir, "src-tauri", "tauri.conf.json"), JSON.stringify({ version: "1.2.3" }));
  if (repository) {
    git(rootDir, "init", "-b", "test-branch");
    git(rootDir, "config", "user.name", "Build Info Test");
    git(rootDir, "config", "user.email", "build-info@example.invalid");
    writeFileSync(path.join(rootDir, "tracked.txt"), "clean\n");
    git(rootDir, "add", "tracked.txt", "src-tauri/tauri.conf.json");
    git(rootDir, "commit", "-m", "fixture");
  }
  return rootDir;
}

test("collects clean commit, branch, tag, version, and reproducible timestamp", () => {
  const rootDir = fixture();
  try {
    git(rootDir, "tag", "v1.2.3");
    const info = collectBuildInfo({
      rootDir,
      profile: "release",
      sourceDateEpoch: "1234567890",
    });
    assert.equal(info.version, "1.2.3");
    assert.match(info.commit, /^[0-9a-f]{40}$/);
    assert.equal(info.shortCommit, info.commit.slice(0, 12));
    assert.equal(info.branch, "test-branch");
    assert.equal(info.tag, "v1.2.3");
    assert.equal(info.dirty, false);
    assert.equal(info.builtAt, "2009-02-13T23:31:30.000Z");
    assert.equal(info.profile, "release");
  } finally {
    rmSync(rootDir, { recursive: true, force: true });
  }
});

test("marks tracked and untracked working-tree changes dirty", () => {
  const rootDir = fixture();
  try {
    writeFileSync(path.join(rootDir, "tracked.txt"), "changed\n");
    assert.equal(collectBuildInfo({ rootDir, profile: "debug" }).dirty, true);
    git(rootDir, "checkout", "--", "tracked.txt");
    writeFileSync(path.join(rootDir, "untracked.txt"), "new\n");
    assert.equal(collectBuildInfo({ rootDir, profile: "debug" }).dirty, true);
  } finally {
    rmSync(rootDir, { recursive: true, force: true });
  }
});

test("reports detached heads without a branch", () => {
  const rootDir = fixture();
  try {
    git(rootDir, "checkout", "--detach", "HEAD");
    const info = collectBuildInfo({ rootDir, profile: "development" });
    assert.equal(info.branch, null);
    assert.match(info.commit, /^[0-9a-f]{40}$/);
  } finally {
    rmSync(rootDir, { recursive: true, force: true });
  }
});

test("does not fail when Git metadata is unavailable", () => {
  const rootDir = fixture({ repository: false });
  try {
    const info = collectBuildInfo({ rootDir, profile: "release", now: 0 });
    assert.equal(info.version, "1.2.3");
    assert.equal(info.commit, "unknown");
    assert.equal(info.shortCommit, "unknown");
    assert.equal(info.branch, null);
    assert.equal(info.tag, null);
    assert.equal(info.dirty, null);
    assert.equal(info.builtAt, "1970-01-01T00:00:00.000Z");
  } finally {
    rmSync(rootDir, { recursive: true, force: true });
  }
});
