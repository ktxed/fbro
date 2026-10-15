import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

export const run = promisify(execFile);
export const toolDir = path.dirname(fileURLToPath(import.meta.url));
export const repoRoot = path.resolve(toolDir, "../..");
export const sentence = "Showcase: browse, organize, and review files in fbro.";
export const commitMessage = "docs(showcase): describe the demo workflow";
export const showcaseZoomSteps = [14, 15, 16];
export const showcaseFontSize = showcaseZoomSteps.at(-1);
export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export async function until(label, predicate, timeout = 20000) {
  const deadline = Date.now() + timeout;
  do {
    const result = await predicate();
    if (result) return result;
    await sleep(150);
  } while (Date.now() < deadline);
  throw new Error(`Timed out waiting for ${label}`);
}

export function within(root, target) {
  const relative = path.relative(root, target);
  return relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative));
}

// Resolve existing ancestors too: a symlink in a not-yet-created path must not
// let fixture setup escape its approved workspace.
export function canonical(target) {
  const absolute = path.resolve(target);
  const leaf = fs.lstatSync(absolute, { throwIfNoEntry: false });
  if (leaf?.isSymbolicLink() && !fs.existsSync(absolute)) throw new Error(`Refusing dangling symlink: ${target}`);
  if (fs.existsSync(absolute)) return fs.realpathSync(absolute);
  if (fs.lstatSync(path.dirname(absolute), { throwIfNoEntry: false }) === undefined) {
    return path.join(canonical(path.dirname(absolute)), path.basename(absolute));
  }
  return path.join(fs.realpathSync(path.dirname(absolute)), path.basename(absolute));
}

export function guardedChild(root, target) {
  const base = canonical(root);
  const actual = canonical(target);
  if (actual === base || !within(base, actual)) throw new Error(`Refusing destination outside demo workspace: ${target}`);
  return actual;
}

export function assetPath(config, name) {
  if (typeof name !== "string" || name !== path.basename(name) || name === "." || name === "..") {
    throw new Error(`Asset must be a filename in pics: ${name}`);
  }
  const target = guardedChild(config.pics, path.join(config.pics, name));
  const stat = fs.lstatSync(target);
  if (!stat.isFile()) throw new Error(`Not a regular media file: ${target}`);
  return target;
}

export function loadConfig(filename = path.join(toolDir, "config.json"), overrides = {}) {
  const config = { ...JSON.parse(fs.readFileSync(filename, "utf8")), ...overrides };
  config.pics = canonical(config.pics);
  config.app = path.resolve(config.app);
  if (!Array.isArray(config.photos) || config.photos.length !== 6 || new Set(config.photos).size !== 6) {
    throw new Error("Configure exactly six distinct JPEG photos");
  }
  if (!Number.isFinite(config.pace) || config.pace < 0.1 || config.pace > 5) throw new Error("pace must be between 0.1 and 5");
  if (!Number.isInteger(config.timeoutMs) || config.timeoutMs < 1000) throw new Error("timeoutMs must be at least 1000");
  if (!Number.isInteger(config.screenIndex) || config.screenIndex < 0) throw new Error("screenIndex must be a nonnegative integer");
  if (!Number.isInteger(config.windowWidth) || config.windowWidth < 800) throw new Error("windowWidth must be at least 800");
  if (!config.anchors || typeof config.anchors !== "object" || Array.isArray(config.anchors)) throw new Error("anchors must be an object");
  for (const [name, anchor] of Object.entries(config.anchors)) {
    if (![anchor.x, anchor.y, anchor.windowWidth, anchor.windowHeight].every(Number.isFinite)
        || anchor.x < 0 || anchor.x >= 1 || anchor.y < 0 || anchor.y >= 1 || anchor.windowWidth <= 0 || anchor.windowHeight <= 0) throw new Error(`Invalid calibration anchor: ${name}`);
  }
  if (!within(repoRoot, canonical(path.resolve(repoRoot, config.markdown)))) throw new Error("Markdown must stay in the fbro checkout");
  for (const name of [...config.photos, config.gif, config.video]) assetPath(config, name);
  if (config.photos.some((name) => !/\.jpe?g$/i.test(name))) throw new Error("Photo assets must be JPEGs");
  return config;
}

export async function git(root, args, options = {}) {
  const { stdout } = await run("git", ["-C", root, ...args], {
    ...options, maxBuffer: 16 * 1024 * 1024,
    env: { ...process.env, GIT_OPTIONAL_LOCKS: "0", ...options.env },
  });
  return stdout.trimEnd();
}

export async function sourceBaseline(root = repoRoot) {
  return {
    head: await git(root, ["rev-parse", "HEAD"]),
    branch: await git(root, ["branch", "--show-current"]),
    refs: await git(root, ["show-ref"]),
    status: await git(root, ["status", "--porcelain=v1", "--untracked-files=all"]),
    diff: await git(root, ["diff", "--binary", "HEAD"]),
    index: hashFile(path.join(root, ".git/index")),
  };
}

export function hashFile(filename) {
  return fs.existsSync(filename) ? crypto.createHash("sha256").update(fs.readFileSync(filename)).digest("hex") : null;
}

export function verifyBaseline(before, after) {
  for (const key of Object.keys(before)) {
    if (before[key] !== after[key]) throw new Error(`Original checkout changed during the run (${key}); inspect it before recording again`);
  }
}

export function captureGeometry(stage, display) {
  if (![stage.x, stage.y, stage.width, stage.height, display.x, display.y, display.width, display.height, display.pixelWidth, display.pixelHeight].every(Number.isFinite)
      || stage.width <= 0 || stage.height <= 0 || display.width <= 0 || display.height <= 0
      || display.pixelWidth <= 0 || display.pixelHeight <= 0) throw new Error("Invalid capture geometry");
  if (stage.x < display.x || stage.y < display.y || stage.x + stage.width > display.x + display.width || stage.y + stage.height > display.y + display.height) {
    throw new Error("The entire fbro stage must fit on the selected display");
  }
  const even = (n) => Math.floor(n / 2) * 2;
  const sx = display.pixelWidth / display.width;
  const sy = display.pixelHeight / display.height;
  return { x: even((stage.x - display.x) * sx), y: even((stage.y - display.y) * sy), width: even(stage.width * sx), height: even(stage.height * sy) };
}

export function ffmpegArgs(config, geometry, output) {
  return ["-hide_banner", "-nostats", "-f", "avfoundation", "-framerate", "30", "-capture_cursor", "1",
    "-i", `Capture screen ${config.screenIndex}:none`, "-an", "-vf",
    `crop=${geometry.width}:${geometry.height}:${geometry.x}:${geometry.y},scale=1920:1080:force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos,pad=1920:1080:(ow-iw)/2:(oh-ih)/2`,
    "-r", "30", "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-n", output];
}

export function writeIndex(manifest, directory) {
  fs.writeFileSync(path.join(directory, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  const stamp = (seconds) => `${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(1).padStart(4, "0")}`;
  const lines = ["# fbro showcase clips", "", `Run: ${manifest.runId}`, "", `Mode: ${manifest.mode}. Source revision: ${manifest.sourceRevision}.`, "",
    "Silent footage with the real cursor. Action times are relative to the start of each clip.", ""];
  for (const clip of manifest.clips) {
    lines.push(`## ${clip.id} — ${clip.title}`, "", `File: ${clip.file ?? "No recording (rehearsal)"} · Status: ${clip.status} · Duration: ${(clip.durationSeconds ?? 0).toFixed(1)}s`, "", clip.description, "");
    if (clip.error) lines.push(`Failure: ${clip.error}`, "");
    if (clip.commitHash) lines.push(`Demo commit: ${clip.commitHash}`, "");
    for (const action of clip.actions) lines.push(`- ${stamp(action.atSeconds)} — ${action.label}`);
    lines.push("");
  }
  if (manifest.error) lines.push(`Run failure: ${manifest.error}`, "");
  fs.writeFileSync(path.join(directory, "clips.md"), lines.join("\n"));
}
