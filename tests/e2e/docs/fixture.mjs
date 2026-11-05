// Demo files for the user-guide screenshots. Everything lives under
// /work/a/demo inside the backend container (artifacts/e2e/fbro-a/work/demo on
// the host), with fixed modification times so the Date column never changes.
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { backendPath, hostPath } from "../smoke/config.mjs";

export const demo = {
  root: backendPath("a", "demo"),
  website: backendPath("a", "demo", "website"),
  photos: backendPath("a", "demo", "photos"),
  backup: backendPath("a", "demo", "backup"),
  backupOctober: backendPath("a", "demo", "backup", "website-2026-10"),
  photosCopy: backendPath("a", "demo", "photos-copy"),
  bPhotos: backendPath("b", "demo", "photos"),
  big: backendPath("a", "demo-large"),
  repo: backendPath("a", "demo", "garden-app"),
  archive: backendPath("a", "demo", "backup", "photos-2026.zip"),
};

const README = `# Lighthouse website

The static site for the Lighthouse community garden: pages, styles and photos.

## Build

\`\`\`sh
npm install
npm run build
\`\`\`

## How a page is published

\`\`\`mermaid
flowchart LR
  A[Markdown page] --> B[Build]
  B --> C{Checks pass?}
  C -- yes --> D[Publish]
  C -- no --> E[Fix and retry]
  E --> B
\`\`\`

## Folders

| Folder | Contents |
|---|---|
| \`src/\` | Page templates and scripts |
| \`docs/\` | Guides for volunteers |
| \`assets/\` | Logo and photos |
`;

const TEXT_FILES = {
  "website/README.md": README,
  "website/package.json": JSON.stringify({ name: "lighthouse-site", version: "1.4.0", private: true,
    scripts: { build: "eleventy", serve: "eleventy --serve" } }, null, 2) + "\n",
  "website/index.html": "<!doctype html>\n<html lang=\"en\">\n<head><title>Lighthouse</title>\n<link rel=\"stylesheet\" href=\"styles.css\"></head>\n<body><h1>Lighthouse community garden</h1></body>\n</html>\n",
  "website/styles.css": "body {\n  font-family: system-ui, sans-serif;\n  margin: 0 auto;\n  max-width: 48rem;\n}\n",
  "website/CHANGELOG.md": "# Changelog\n\n## 1.4.0\n- Add the volunteer calendar.\n\n## 1.3.0\n- New photo gallery.\n",
  "website/LICENSE": "MIT License\n\nCopyright (c) 2026 Lighthouse volunteers\n",
  "website/notes.txt": "Ask Sam about the spring planting dates.\nReplace the hero photo before May.\n",
  "website/src/app.js": "export function start() {\n  console.log(\"Lighthouse\");\n}\n",
  "website/src/calendar.js": "export const events = [];\n",
  "website/src/gallery.js": "export function gallery(photos) {\n  return photos.map((p) => p.src);\n}\n",
  "website/docs/volunteers.md": "# Volunteer guide\n\nWelcome!\n",
  "website/docs/planting.md": "# Planting calendar\n",
  "backup/website-2026-10/CHANGELOG.md": "# Changelog\n\n## 1.5.0\n- Dark mode for the photo gallery.\n\n## 1.4.0\n- Add the volunteer calendar and email reminders.\n\n## 1.3.0\n- New photo gallery.\n",
  "backup/notes-2026-09.txt": "Old notes.\n",
  "backup/README.md": "# Backups\n\nMonthly copies of the website.\n",
};
const FOLDERS = ["website/src", "website/docs", "website/assets", "backup/website-2026-09", "backup/website-2026-10", "backup/website-2026-10/assets", "photos"];

// Fixed timestamps: 2026-10-01 09:00 UTC plus a few hours per file.
const BASE_MTIME = Date.UTC(2026, 9, 1, 9, 0, 0) / 1000;

function setTime(file, index) {
  const t = BASE_MTIME + index * 5 * 3600 + (index % 7) * 1100;
  fs.utimesSync(file, t, t);
}

/** Landscape-like JPEG/PNG pixels drawn in the app's own WebView. */
async function renderImages(browser, specs) {
  return browser.execute((specs) => specs.map(({ w, h, hue, type }) => {
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    const sky = ctx.createLinearGradient(0, 0, 0, h);
    sky.addColorStop(0, `hsl(${hue}, 60%, 70%)`);
    sky.addColorStop(1, `hsl(${(hue + 30) % 360}, 70%, 88%)`);
    ctx.fillStyle = sky;
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = `hsl(${(hue + 40) % 360}, 90%, 85%)`;
    ctx.beginPath();
    ctx.arc(w * 0.72, h * 0.3, h * 0.11, 0, Math.PI * 2);
    ctx.fill();
    for (let layer = 0; layer < 3; layer++) {
      ctx.fillStyle = `hsl(${(hue + 90 + layer * 15) % 360}, ${35 + layer * 10}%, ${45 - layer * 12}%)`;
      ctx.beginPath();
      ctx.moveTo(0, h);
      for (let x = 0; x <= w; x += w / 24) {
        const y = h * (0.55 + layer * 0.12) + Math.sin(x / w * (3 + layer) * Math.PI + hue) * h * 0.06;
        ctx.lineTo(x, y);
      }
      ctx.lineTo(w, h);
      ctx.closePath();
      ctx.fill();
    }
    return canvas.toDataURL(type, 0.85).split(",")[1];
  }), specs);
}

export async function createDemoFixture(browser) {
  const root = hostPath(demo.root);
  fs.rmSync(root, { recursive: true, force: true });
  for (const folder of FOLDERS) fs.mkdirSync(path.join(root, folder), { recursive: true });
  for (const [name, content] of Object.entries(TEXT_FILES)) fs.writeFileSync(path.join(root, name), content);

  const photos = [
    ["IMG_0412.jpg", 20], ["IMG_0413.jpg", 200], ["IMG_0418.jpg", 35], ["IMG_0420.jpg", 260],
    ["IMG_0431.jpg", 330], ["IMG_0437.jpg", 180], ["IMG_0440.jpg", 10], ["IMG_0452.jpg", 220],
    ["garden-map.png", 120], ["poster.png", 290],
  ];
  const assets = [["website/assets/hero.jpg", 30], ["website/assets/logo.png", 210]];
  const specs = [...photos.map(([name, hue]) => ({ name: `photos/${name}`, hue })), ...assets.map(([name, hue]) => ({ name, hue }))]
    .map((spec) => ({ ...spec, w: 1600, h: 1067, type: spec.name.endsWith(".png") ? "image/png" : "image/jpeg" }));
  const images = await renderImages(browser, specs);
  specs.forEach((spec, i) => fs.writeFileSync(path.join(root, spec.name), Buffer.from(images[i], "base64")));

  // Copies of two photos, so the duplicate finder has groups to show.
  for (const name of ["IMG_0412.jpg", "IMG_0418.jpg"]) {
    fs.copyFileSync(path.join(root, "photos", name), path.join(root, "backup/website-2026-10/assets", name));
  }
  // Sparse, so it costs no disk and no time to create.
  // It lives outside demo/ so Fedup and hashing scenes over demo/ skip it.
  const big = hostPath(demo.big);
  fs.rmSync(big, { recursive: true, force: true });
  fs.mkdirSync(big, { recursive: true });
  fs.closeSync(fs.openSync(path.join(big, "footage-2026.mov"), "w"));
  fs.truncateSync(path.join(big, "footage-2026.mov"), 6 * 1024 ** 3);

  // Timestamps last, children before their folders.
  const all = [];
  const walk = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      all.push(full);
    }
  };
  walk(root);
  all.forEach((file, i) => setTime(file, i));

  createArchive(root);
  createRepository(root);
}

/** photos-2026.zip next to the photos, so a zip offers "Browse archive". */
function createArchive(root) {
  const staging = fs.mkdtempSync(path.join(root, ".zip-"));
  fs.mkdirSync(path.join(staging, "photos-2026"));
  for (const name of ["IMG_0440.jpg", "IMG_0452.jpg"]) {
    fs.copyFileSync(path.join(root, "photos", name), path.join(staging, "photos-2026", name));
  }
  fs.writeFileSync(path.join(staging, "photos-2026", "credits.txt"), "Photos by the Lighthouse volunteers.\n");
  const out = path.join(root, "backup/photos-2026.zip");
  execFileSync("zip", ["-qrX", out, "photos-2026"], { cwd: staging });
  fs.rmSync(staging, { recursive: true, force: true });
  const t = BASE_MTIME + 90 * 3600;
  fs.utimesSync(out, t, t);
}

const AUTHOR_DATES = ["2026-09-14T10:00:00Z", "2026-09-21T16:30:00Z", "2026-09-29T09:15:00Z"];

/** demo/garden-app: three commits, then a modified, a staged and an untracked file. */
function createRepository(root) {
  const repo = path.join(root, "garden-app");
  fs.rmSync(repo, { recursive: true, force: true });
  fs.mkdirSync(path.join(repo, "src"), { recursive: true });
  const git = (args, date) => execFileSync("git", ["-C", repo, ...args], {
    stdio: "pipe",
    env: { ...process.env,
      GIT_AUTHOR_NAME: "Lighthouse volunteer", GIT_AUTHOR_EMAIL: "volunteer@example.org",
      GIT_COMMITTER_NAME: "Lighthouse volunteer", GIT_COMMITTER_EMAIL: "volunteer@example.org",
      ...(date ? { GIT_AUTHOR_DATE: date, GIT_COMMITTER_DATE: date } : {}) },
  });
  const write = (name, text) => fs.writeFileSync(path.join(repo, name), text);
  git(["init", "-q", "-b", "main"]);
  write("README.md", "# Garden app\n\nPlan beds and track watering.\n");
  write("src/beds.js", "export const beds = [];\n");
  git(["add", "."]);
  git(["commit", "-qm", "Start the garden app"], AUTHOR_DATES[0]);
  write("src/watering.js", "export function nextWatering(bed) {\n  return bed.lastWatered + 2;\n}\n");
  git(["add", "."]);
  git(["commit", "-qm", "Add a watering schedule\n\nBeds are watered every two days unless it rained."], AUTHOR_DATES[1]);
  write("src/beds.js", "export const beds = [\n  { name: \"Tomatoes\", lastWatered: 0 },\n];\n");
  write("README.md", "# Garden app\n\nPlan beds and track watering.\n\n## Run\n\nOpen index.html.\n");
  git(["add", "."]);
  git(["commit", "-qm", "List the first bed and document how to run"], AUTHOR_DATES[2]);

  write("src/beds.js", "export const beds = [\n  { name: \"Tomatoes\", lastWatered: 0 },\n  { name: \"Beans\", lastWatered: 1 },\n];\n");
  write("src/watering.js", "export function nextWatering(bed, rained = false) {\n  return bed.lastWatered + (rained ? 4 : 2);\n}\n");
  git(["add", "src/watering.js"]);
  write("TODO.md", "- Add a bed editor\n- Remind volunteers by email\n");

  const t = BASE_MTIME + 120 * 3600;
  for (const file of ["README.md", "TODO.md", "src/beds.js", "src/watering.js", "src", "."]) fs.utimesSync(path.join(repo, file), t, t);
}
