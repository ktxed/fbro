import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { assetPath, canonical, commitMessage, git, guardedChild, hashFile, repoRoot, run, sentence } from "./core.mjs";

export async function makeDemoClone(destination, revision) {
  if (fs.existsSync(destination)) throw new Error(`Demo clone must have a fresh destination: ${destination}`);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  // --no-local transfers objects rather than sharing files or alternates.
  await run("git", ["clone", "--no-local", "--no-checkout", "--", repoRoot, destination], { maxBuffer: 16 * 1024 * 1024 });
  await git(destination, ["remote", "remove", "origin"]);
  const hooks = path.join(destination, ".git/showcase-empty-hooks");
  fs.mkdirSync(hooks);
  for (const [key, value] of Object.entries({ "user.name": "fbro Demo", "user.email": "showcase@example.invalid", "commit.gpgsign": "false", "core.hooksPath": hooks, "core.autocrlf": "false", "core.eol": "lf" })) {
    await git(destination, ["config", "--local", key, value]);
  }
  await git(destination, ["switch", "-c", "demo/showcase", revision]);
  await assertDemoClone(destination, revision);
}

export async function assertDemoClone(destination, revision) {
  const root = canonical(destination);
  assert.equal(canonical(await git(root, ["rev-parse", "--show-toplevel"])), root, "Git action escaped demo root");
  assert.equal(canonical(await git(root, ["rev-parse", "--absolute-git-dir"])), path.join(root, ".git"), "Demo must own its Git directory");
  assert.equal(await git(root, ["rev-parse", "--git-common-dir"]), ".git", "Shared Git metadata is not allowed");
  assert.equal(await git(root, ["remote"]), "", "Demo clone must have no remotes");
  assert.equal(await git(root, ["branch", "--show-current"]), "demo/showcase");
  assert.equal(fs.existsSync(path.join(root, ".git/objects/info/alternates")), false, "Shared object stores are not allowed");
  if (revision) assert.equal(await git(root, ["rev-parse", "HEAD"]), revision, "Unexpected demo HEAD");
}

export function readmeEdit(filename) {
  const original = fs.readFileSync(filename, "utf8");
  if (original.includes(sentence)) throw new Error("Demo README already contains the showcase edit");
  const anchor = "## Design docs\n";
  if (original.split(anchor).length !== 2) throw new Error("Demo README must contain one Design docs heading");
  const offset = original.indexOf(anchor);
  return `${original.slice(0, offset)}${sentence}\n\n${original.slice(offset)}`;
}

export async function assertPendingReadme(fixture, { staged = false } = {}) {
  await assertDemoClone(fixture.clone, fixture.revision);
  assert.equal(fs.readFileSync(path.join(fixture.clone, "README.md"), "utf8"), fixture.expectedReadme, "Unexpected README contents");
  assert.equal(await git(fixture.clone, ["status", "--porcelain=v1", "--untracked-files=all"]), `${staged ? "M " : " M"} README.md`);
  assert.equal(await git(fixture.clone, ["diff", ...(staged ? ["--cached"] : []), "--name-only"]), "README.md");
}

export async function verifyDemoCommit(fixture) {
  await assertDemoClone(fixture.clone);
  const hash = await git(fixture.clone, ["rev-parse", "HEAD"]);
  assert.notEqual(hash, fixture.revision);
  assert.equal(await git(fixture.clone, ["rev-parse", "HEAD^"]), fixture.revision, "Expected exactly one demo commit");
  assert.equal(await git(fixture.clone, ["log", "-1", "--format=%s"]), commitMessage);
  assert.equal(await git(fixture.clone, ["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"]), "README.md");
  assert.equal(await git(fixture.clone, ["status", "--porcelain=v1", "--untracked-files=all"]), "", "Demo clone is not clean after commit");
  assert.equal(await git(fixture.clone, ["show", "HEAD:README.md"]), fixture.expectedReadme.trimEnd());
  return hash;
}

export async function prepareFixture(id, context) {
  const { config, runId, directory, revision } = context;
  const demoRoot = guardedChild(config.pics, path.join(config.pics, "fbro-demo", runId));
  const workspace = guardedChild(demoRoot, path.join(demoRoot, id));
  const fixture = { id, workspace, revision, photos: [...config.photos], shortlist: path.join(workspace, "Shortlist") };
  if (["02-collect-shortlist", "03-remove-rejects", "04-rename-keeper", "06-media-viewers", "07-compare-notes", "11-select-by-mask", "12-keyboard", "13-color-schemes", "14-editor-tabs", "15-panel-tabs", "16-permissions", "17-job-files"].includes(id)) {
    fs.mkdirSync(workspace, { recursive: true });
  }
  if (id === "11-select-by-mask") {
    const photos = [[config.photos[0], "photo-a.jpg"], [config.photos[1], "photo-b.jpg"], [config.photos[2], "slide-c.jpg"]];
    for (const [source, name] of photos) fs.copyFileSync(assetPath(config, source), guardedChild(workspace, path.join(workspace, name)), fs.constants.COPYFILE_EXCL);
    fs.writeFileSync(path.join(workspace, "notes.txt"), "Notes that a JPEG mask must leave alone.\n");
    fs.writeFileSync(path.join(workspace, "readme.md"), "A markdown file outside *.jpg.\n");
    fixture.maskPhotos = photos.map(([, name]) => name);
  }
  if (id === "12-keyboard") {
    fs.copyFileSync(assetPath(config, config.photos[0]), guardedChild(workspace, path.join(workspace, "view.jpg")), fs.constants.COPYFILE_EXCL);
    fs.writeFileSync(path.join(workspace, "readme.txt"), "Button bar demo.\n");
  }
  if (id === "13-color-schemes") {
    for (const name of fixture.photos) fs.copyFileSync(assetPath(config, name), guardedChild(workspace, path.join(workspace, name)), fs.constants.COPYFILE_EXCL);
  }
  if (id === "14-editor-tabs") {
    fs.writeFileSync(path.join(workspace, "SHA256SUMS"), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  photo-a.jpg\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  photo-b.jpg\n");
    fs.writeFileSync(path.join(workspace, "shortlist.js"), "const keep = [\"photo-a.jpg\", \"photo-b.jpg\", \"slide-c.jpg\"];\nconst caption = \"A deliberately long caption line for the showcase editor so wrapping is obvious when the margin is narrow and trailing spaces stay visible.   \";\nexport function shortlist() {\n  return keep;\n}\n");
    fs.writeFileSync(path.join(workspace, "showcase.md"), "# Showcase notes\n\nA short note rendered beside the editor.\n");
    fixture.editorMark = "// Showcase edit";
    fixture.noteMark = "Captured in fbro.";
  }
  if (id === "15-panel-tabs") {
    fixture.album = guardedChild(workspace, path.join(workspace, "Album"));
    fs.mkdirSync(fixture.album);
    fixture.folders = ["Alpha", "Bravo", "Charlie", "Delta"];
    for (const name of fixture.folders) {
      const dir = guardedChild(fixture.album, path.join(fixture.album, name));
      fs.mkdirSync(dir);
      fs.writeFileSync(path.join(dir, "readme.txt"), `${name}\n`);
    }
  }
  if (id === "16-permissions") {
    const script = path.join(workspace, "present.sh");
    fs.writeFileSync(script, "#!/bin/sh\necho showcase\n");
    fs.chmodSync(script, 0o644);
  }
  if (id === "17-job-files") {
    fs.writeFileSync(path.join(workspace, "keeper-notes.txt"), "Keep the cover photo with the shortlist.\n");
    fs.writeFileSync(path.join(workspace, "cover-plan.txt"), "Rename the chosen photo before the presentation.\n");
    fixture.incoming = path.join(workspace, "Incoming");
    fixture.copied = ["keeper-notes.txt", "cover-plan.txt"];
  }
  if (["03-remove-rejects", "04-rename-keeper"].includes(id)) {
    fs.mkdirSync(fixture.shortlist);
    const photos = id === "04-rename-keeper" ? fixture.photos.slice(3) : fixture.photos;
    for (const name of photos) fs.copyFileSync(assetPath(config, name), guardedChild(workspace, path.join(fixture.shortlist, name)), fs.constants.COPYFILE_EXCL);
  }
  if (id === "06-media-viewers") {
    const media = [[config.photos[0], "01-photo.jpg"], [config.photos[1], "02-photo.jpg"], [config.gif, "03-animation.gif"], [config.video, "04-video.mov"]];
    for (const [source, name] of media) fs.copyFileSync(assetPath(config, source), guardedChild(workspace, path.join(workspace, name)), fs.constants.COPYFILE_EXCL);
  }
  if (id === "07-compare-notes") {
    fs.writeFileSync(path.join(workspace, "shortlist-notes-v1.txt"), "Photo shortlist\n\nKeep six photos.\nInclude every candidate.\nReview filenames later.\n");
    fs.writeFileSync(path.join(workspace, "shortlist-notes-v2.txt"), "Photo shortlist\n\nKeep three photos.\nRemove the rejected candidates.\nRename the cover photo.\nReady for the presentation.\n");
  }
  if (["09-edit-review", "10-stage-commit"].includes(id)) {
    fixture.clone = guardedChild(directory, path.join(directory, id, "fbro-demo"));
    await makeDemoClone(fixture.clone, revision);
    fixture.originalReadme = fs.readFileSync(path.join(fixture.clone, "README.md"), "utf8");
    fixture.expectedReadme = readmeEdit(path.join(fixture.clone, "README.md"));
    fixture.readmeInsertText = `${sentence}\n\n`;
    if (id === "10-stage-commit") fs.writeFileSync(path.join(fixture.clone, "README.md"), fixture.expectedReadme);
  }
  return fixture;
}

export function mediaBaseline(config) {
  return Object.fromEntries([...config.photos, config.gif, config.video].map((name) => [name, hashFile(assetPath(config, name))]));
}
