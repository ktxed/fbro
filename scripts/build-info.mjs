import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";

function runGit(rootDir, args) {
  try {
    return {
      ok: true,
      value: execFileSync("git", ["-C", rootDir, ...args], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      }).trim(),
    };
  } catch {
    return { ok: false, value: "" };
  }
}

function readVersion(rootDir) {
  try {
    const configPath = path.join(rootDir, "src-tauri", "tauri.conf.json");
    const config = JSON.parse(readFileSync(configPath, "utf8"));
    return typeof config.version === "string" ? config.version : "unknown";
  } catch {
    return "unknown";
  }
}

function buildTimestamp(sourceDateEpoch, now) {
  if (sourceDateEpoch !== undefined && sourceDateEpoch !== "") {
    const seconds = Number(sourceDateEpoch);
    if (Number.isFinite(seconds) && seconds >= 0) {
      return new Date(seconds * 1000).toISOString();
    }
  }
  return new Date(now).toISOString();
}

export function collectBuildInfo({
  rootDir,
  profile,
  sourceDateEpoch = process.env.SOURCE_DATE_EPOCH,
  now = Date.now(),
}) {
  const commitResult = runGit(rootDir, ["rev-parse", "HEAD"]);
  const branchResult = runGit(rootDir, ["symbolic-ref", "--short", "-q", "HEAD"]);
  const tagResult = runGit(rootDir, ["describe", "--tags", "--exact-match", "HEAD"]);
  const statusResult = runGit(rootDir, ["status", "--porcelain", "--untracked-files=normal"]);
  const commit = commitResult.ok && commitResult.value ? commitResult.value : "unknown";

  return {
    version: readVersion(rootDir),
    commit,
    shortCommit: commit === "unknown" ? "unknown" : commit.slice(0, 12),
    branch: branchResult.ok && branchResult.value ? branchResult.value : null,
    tag: tagResult.ok && tagResult.value ? tagResult.value : null,
    dirty: statusResult.ok ? statusResult.value.length > 0 : null,
    builtAt: buildTimestamp(sourceDateEpoch, now),
    profile,
  };
}
