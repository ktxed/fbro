#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { repoRoot, run, within } from "./core.mjs";

async function probe(filename) {
  const { stdout } = await run("ffprobe", ["-v", "error", "-show_streams", "-show_format", "-of", "json", filename]);
  const result = JSON.parse(stdout);
  const video = result.streams.find(stream => stream.codec_type === "video");
  const duration = Number(result.format.duration);
  if (!video || !Number.isFinite(duration) || duration <= 0) throw new Error(`Not a valid video: ${filename}`);
  return { video, duration, audio: result.streams.some(stream => stream.codec_type === "audio") };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  async function main() {
    const { values } = parseArgs({ options: {
      input: { type: "string" }, output: { type: "string" }, speed: { type: "string", default: "5" },
      help: { type: "boolean", short: "h" },
    } });
    if (values.help) {
      process.stdout.write("Usage: node tools/showcase/montage.mjs [--input RUN_FOLDER] [--speed 5] [--output FILE.mp4]\n\nDefaults to the latest complete clip set at 5x speed. Original clips are preserved.\n");
      return;
    }
    await montage({ input: values.input, output: values.output, speed: Number(values.speed) });
  }
  main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}

export async function montage({ input, output, speed = 5 } = {}) {
  if (!Number.isFinite(speed) || speed <= 0 || speed > 100) throw new Error("--speed must be greater than 0 and at most 100");
  const directory = fs.realpathSync(input ?? fs.readFileSync(path.join(repoRoot, "artifacts/showcase/latest-complete.txt"), "utf8").trim());
  const manifest = JSON.parse(fs.readFileSync(path.join(directory, "manifest.json"), "utf8"));
  if (!manifest.clips?.length || manifest.clips.some(clip => clip.status !== "passed" || !clip.file)) {
    throw new Error("Choose a recorded run containing only passed clips, or a collected final set");
  }
  const files = manifest.clips.map(clip => {
    if (clip.file !== path.basename(clip.file) || /[\r\n]/.test(clip.file)) throw new Error(`Invalid clip filename: ${clip.file}`);
    const filename = fs.realpathSync(path.join(directory, clip.file));
    if (!within(directory, filename) || /[\r\n]/.test(filename)) throw new Error(`Clip escaped the input folder: ${clip.file}`);
    return filename;
  });
  const sources = await Promise.all(files.map(probe));
  const first = sources[0].video;
  if (sources.some(({ video }) => ["codec_name", "width", "height", "pix_fmt", "r_frame_rate", "time_base"].some(key => video[key] !== first[key]))) {
    throw new Error("The recorded clips must have matching codecs, dimensions, pixel formats, frame rates, and time bases");
  }
  const destination = path.resolve(output ?? path.join(directory, `showcase-${speed}x.mp4`));
  if (path.extname(destination).toLowerCase() !== ".mp4") throw new Error("--output must be an .mp4 filename");
  const { dir, name } = path.parse(destination);
  const jsonFile = path.join(dir, `${name}.json`);
  const indexFile = path.join(dir, `${name}.md`);
  for (const filename of [destination, jsonFile, indexFile]) {
    if (fs.existsSync(filename)) throw new Error(`Output already exists; choose another --output: ${filename}`);
  }
  fs.mkdirSync(dir, { recursive: true });
  let elapsed = 0;
  const chapters = manifest.clips.map((clip, index) => {
    const chapter = { id: clip.id, title: clip.title, source: files[index], sourceDurationSeconds: sources[index].duration,
      startSeconds: elapsed / speed, endSeconds: (elapsed + sources[index].duration) / speed,
      actions: clip.actions.map(action => ({ ...action, sourceAtSeconds: action.atSeconds, atSeconds: (elapsed + action.atSeconds) / speed })) };
    elapsed += sources[index].duration;
    return chapter;
  });
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "fbro-montage-"));
  const concatFile = path.join(temporary, "clips.ffconcat");
  const metadataFile = path.join(temporary, "chapters.ffmetadata");
  const escapeMetadata = value => value.replace(/[\\=;#\n\r]/g, character => character === "\r" || character === "\n" ? " " : `\\${character}`);
  try {
    fs.writeFileSync(concatFile, `ffconcat version 1.0\n${files.map(filename => `file '${filename.replace(/'/g, "'\\''")}'`).join("\n")}\n`);
    fs.writeFileSync(metadataFile, `;FFMETADATA1\ntitle=fbro showcase (${speed}x)\n${chapters.map(chapter =>
      `[CHAPTER]\nTIMEBASE=1/1000\nSTART=${Math.round(chapter.startSeconds * 1000)}\nEND=${Math.round(chapter.endSeconds * 1000)}\ntitle=${escapeMetadata(chapter.title)}`).join("\n")}\n`);
    process.stdout.write(`Joining ${files.length} clips at ${speed}x: ${elapsed.toFixed(1)}s → approximately ${(elapsed / speed).toFixed(1)}s\n`);
    await run("ffmpeg", ["-hide_banner", "-loglevel", "error", "-nostdin", "-n", "-f", "concat", "-safe", "0", "-i", concatFile,
      "-f", "ffmetadata", "-i", metadataFile, "-map", "0:v:0", "-map_metadata", "1", "-map_chapters", "1", "-an",
      "-vf", `setpts=(PTS-STARTPTS)/${speed}`, "-r", "30", "-fps_mode", "cfr", "-c:v", "libx264", "-preset", "veryfast",
      "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", destination], { maxBuffer: 8 * 1024 * 1024 });
    const result = await probe(destination);
    if (result.video.codec_name !== "h264" || result.audio || Math.abs(result.duration - elapsed / speed) > 0.5) {
      throw new Error("Montage verification failed; inspect the output before using it");
    }
    const report = { sourceRun: manifest.runId, sourceDirectory: directory, file: destination, speed, durationSeconds: result.duration,
      sourceDurationSeconds: elapsed, width: result.video.width, height: result.video.height, fps: result.video.r_frame_rate, audio: false, chapters };
    fs.writeFileSync(jsonFile, `${JSON.stringify(report, null, 2)}\n`, { flag: "wx" });
    const stamp = seconds => `${Math.floor(seconds / 60)}:${(seconds % 60).toFixed(1).padStart(4, "0")}`;
    fs.writeFileSync(indexFile, ["# fbro showcase montage", "", `File: ${path.basename(destination)} · Speed: ${speed}× · Duration: ${result.duration.toFixed(1)}s`, "",
      "Chapter and action times refer to the accelerated video.", "", ...chapters.flatMap(chapter => [
        `## ${stamp(chapter.startSeconds)} — ${chapter.title}`, "", ...chapter.actions.map(action => `- ${stamp(action.atSeconds)} — ${action.label}`), "",
      ])].join("\n"), { flag: "wx" });
    process.stdout.write(`Created: ${destination}\nEditing index: ${indexFile}\n`);
    return report;
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}
