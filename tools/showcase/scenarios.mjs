import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { commitMessage, git, guardedChild, hashFile, repoRoot, showcaseZoomSteps, sleep, until } from "./core.mjs";
import { assertDemoClone, assertPendingReadme, verifyDemoCommit } from "./fixtures.mjs";

export const chapters = [
  ["01-browse-filter", "Customize, browse, and filter", "Open the command palette and About fbro, zoom in three levels while About is visible, then show themes, grid/list views, and filename filtering."],
  ["02-collect-shortlist", "Collect a photo shortlist", "Create a destination folder, select six photos, copy, navigate into the result, and switch between grid and list."],
  ["03-remove-rejects", "Remove rejected photos", "Delete one copied photo in grid mode, return to list to delete two together, and preview the remaining thumbnails."],
  ["04-rename-keeper", "Rename the keepers", "Rename a photo in grid mode, return to list to rename its folder, then preview the renamed collection."],
  ["05-markdown-mermaid", "Read Markdown and Mermaid", "Maximize the Markdown viewer, search and navigate occurrences, scroll with the keyboard in four directions, then render Markdown and expand a Mermaid diagram."],
  ["06-media-viewers", "Preview photos and media", "JPEG zoom and next-file navigation, animated GIF, and MOV playback."],
  ["07-compare-notes", "Compare shortlist notes", "Select two text versions and review the internal side-by-side comparison."],
  ["08-explore-git", "Explore fbro’s Git history", "Inspect status, history, and a historical diff; maximize, navigate down and up, and switch between unified and side-by-side modes."],
  ["09-edit-review", "Edit and review documentation", "Edit README in the isolated clone with embedded Nano, then maximize and navigate the Git diff in side-by-side and unified modes."],
  ["10-stage-commit", "Stage and commit", "Stage only README, commit, and inspect its history and maximized diff with navigation and unified mode."],
].map(([id, title, description]) => ({ id, title, description }));

export function selectChapters(value = "all") {
  const ids = value === "all" ? chapters.map((chapter) => chapter.id) : value.split(",");
  if (!ids.length || new Set(ids).size !== ids.length) throw new Error("Choose distinct chapter IDs, or all");
  const selected = ids.map((id) => {
    const chapter = chapters.find((candidate) => candidate.id === id || candidate.id.startsWith(`${id}-`));
    if (!chapter) throw new Error(`Unknown chapter: ${id}`);
    return chapter;
  });
  if (new Set(selected.map((chapter) => chapter.id)).size !== selected.length) throw new Error("Choose distinct chapters");
  return selected;
}

export function startingPaths(fixture, config) {
  const left = {
    "01-browse-filter": config.pics,
    "02-collect-shortlist": config.pics,
    "03-remove-rejects": fixture.shortlist,
    "04-rename-keeper": fixture.shortlist,
    "05-markdown-mermaid": path.dirname(path.resolve(repoRoot, config.markdown)),
    "06-media-viewers": fixture.workspace,
    "07-compare-notes": fixture.workspace,
    "08-explore-git": repoRoot,
    "09-edit-review": fixture.clone,
    "10-stage-commit": fixture.clone,
  }[fixture.id];
  const right = ["02-collect-shortlist", "04-rename-keeper"].includes(fixture.id) ? fixture.workspace
    : ["09-edit-review", "10-stage-commit", "01-browse-filter"].includes(fixture.id) ? repoRoot : config.pics;
  return { left, right };
}

export class Scene {
  constructor(input, observer, fixture, config, clip) { Object.assign(this, { input, observer, fixture, config, clip }); }

  async step(label, action, hold = 800) {
    this.clip.actions.push({ label, atSeconds: (performance.now() - this.startedAt) / 1000 });
    process.stdout.write(`  ${label}\n`);
    await action();
    await sleep(hold * this.config.pace);
  }

  async select(names, panel = "left") {
    await this.input.focusPanel(panel);
    await this.input.command("panel.selection.clear");
    await this.observer.selection(panel, []);
    const selected = [];
    for (const name of names) {
      await this.input.entry(panel, name);
      await this.input.command("panel.selection.toggle_or_size");
      selected.push(name);
      await this.observer.selection(panel, selected);
    }
  }

  async view(mode, panel = "left") {
    await this.input.focusPanel(panel);
    await this.observer.refresh();
    if (this.observer.panel(panel).viewMode !== mode) await this.input.command("panel.toggle_view_mode");
    await this.observer.wait(`${panel} ${mode} view`, () => this.observer.panel(panel).viewMode === mode);
    await this.input.find({ role: "AXRow" }, { panel });
  }

  async guard(names, panel = "left", expectedPath = this.fixture.shortlist) {
    await this.observer.path(panel, expectedPath);
    guardedChild(this.fixture.workspace, expectedPath);
    await this.observer.selection(panel, names);
    for (const name of names) {
      const filename = guardedChild(this.fixture.workspace, path.join(expectedPath, name));
      if (!fs.lstatSync(filename).isFile()) throw new Error(`Mutation target is not a regular demo file: ${filename}`);
    }
    assert.equal(this.observer.state.focus, panel);
  }

  async operation(label, guard = async () => {}) {
    await guard();
    await this.input.button(label);
    // Foreground JobStarted/JobEvent replies are request-local, and snapshots
    // intentionally omit the job store. The GUI's terminal success message
    // reflects the requesting connection's authoritative Completed event.
    await this.input.find({ label: "Operation completed successfully." });
    await this.input.button("Close");
  }

  async remove(names, viewMode = "list") {
    await this.select(names);
    await this.view(viewMode);
    await this.guard(names);
    await this.input.command("file.delete");
    if (names.length === 1) await this.input.find({ label: `Delete ${names[0]}` });
    else for (const name of names) await this.input.find({ label: name });
    await this.operation("Delete", () => this.guard(names));
    await this.observer.wait("deleted rows removed", () => names.every((name) => !this.observer.panel().entries.some((entry) => entry.name === name)));
    for (const name of names) assert.equal(fs.existsSync(path.join(this.fixture.shortlist, name)), false);
  }

  async viewer(name) {
    await this.input.entry("left", name);
    const state = await this.input.inspect();
    if (state.windows.length !== 1 || state.windows[state.focusedWindow]?.title !== this.input.mainTitle) {
      throw new Error("Viewer opening requires the main listing; another window is already open");
    }
    await this.input.command("file.view");
    await until(`separate viewer for ${name}`, async () => (await this.input.inspect()).windows.length > 1, this.config.timeoutMs);
    await this.input.contain();
    await this.input.find({ label: "Text mode (1)", role: "AXButton" });
  }

  async closeViewer() {
    await this.input.key("Escape");
    await until("viewer closed", async () => (await this.input.inspect()).windows.length === 1, this.config.timeoutMs);
  }

  async imageReady(name) {
    if (name) await until(`viewer title ${name}`, async () => (await this.input.inspect()).windows.some((window) => window.title === `${name} — fbro viewer`), this.config.timeoutMs);
    await this.input.find({ role: "AXImage" });
  }

  async gitModal() {
    await this.input.command("panel.git.actions");
    await this.input.find({ label: "Log", prefix: true, role: "AXRadioButton|AXButton" });
  }

  async expandLatestCommit() {
    const row = await this.input.find({ label: "Commit 1 of ", prefix: true });
    // Click the subject area, away from branch/ref badges and copy buttons.
    // A single click focuses the row; Right expands its commit details.
    await this.input.move(row.frame.x + row.frame.width * 0.25, row.frame.y + row.frame.height / 2, true);
    await this.input.key("ArrowRight");
    await this.input.find({ label: "Change 1 of ", prefix: true });
  }

  async diff() {
    await this.input.button("View diff");
    await until("separate comparison window", async () => (await this.input.inspect()).windows.length > 1, this.config.timeoutMs);
    await this.input.contain();
  }

  async historicalDiff() {
    // Keep expansion, entering the first change, and opening its diff together:
    // a live repository refresh can replace the log rows between mouse clicks.
    await this.expandLatestCommit();
    await this.input.key("ArrowRight");
    await this.input.key("v");
    await until("historical comparison window", async () => (await this.input.inspect()).windows.length > 1, this.config.timeoutMs);
    await this.input.contain();
  }

  async keys(keys) {
    for (const key of keys) { await this.input.key(key); await sleep(120); }
  }

  async maximizeViewer() {
    const before = await this.input.inspect();
    this.viewerFrame = before.windows[before.focusedWindow].frame;
    await this.input.key("m");
    await until("maximized viewer", async () => {
      const state = await this.input.inspect();
      const frame = state.windows[state.focusedWindow].frame;
      return frame.width > this.viewerFrame.width + 20 || frame.height > this.viewerFrame.height + 20;
    }, this.config.timeoutMs);
  }

  async restoreViewer() {
    await this.input.key("m");
    await until("restored viewer", async () => {
      const state = await this.input.inspect();
      const frame = state.windows[state.focusedWindow].frame;
      return Math.abs(frame.width - this.viewerFrame.width) < 2 && Math.abs(frame.height - this.viewerFrame.height) < 2;
    }, this.config.timeoutMs);
  }

  async exploreDiff() {
    await this.step("Maximize the Git diff", () => this.maximizeViewer(), 1200);
    await this.step("Navigate down through the diff", () => this.keys(["ArrowDown", "PageDown"]), 1200);
    await this.step("Navigate back up through the diff", () => this.keys(["PageUp", "ArrowUp"]), 1200);
    await this.step("Switch to unified diff mode", () => this.input.button("Unified view"), 1600);
    await this.step("Scroll down and up in unified mode", () => this.keys(["PageDown", "PageUp"]), 1000);
    await this.step("Return to side-by-side mode", () => this.input.button("Side by side view"), 1200);
    await this.step("Restore the comparison window", () => this.restoreViewer());
  }
}

const flows = {
  async "01-browse-filter"(s) {
    const { input: i, observer: o } = s;
    await s.step("Show the command palette", async () => {
      await i.command("app.command_palette");
      await i.find({ role: "AXTextField|AXComboBox", focused: true });
    }, 1500);
    await s.step("Open About fbro from the command palette", async () => {
      const { command } = o.binding("app.about");
      await i.type(command.title);
      await i.click({ label: `${command.title} ${command.category}` });
      await i.find({ label: "Copy build info", role: "AXButton" });
    }, 1500);
    await s.step("Press Cmd+Plus three times to zoom to 16px", async () => {
      for (const target of showcaseZoomSteps) {
        await i.key("=", ["cmd", "shift"]);
        await o.wait(`zoom to ${target}px`, (state) => state.fontSize === target);
        await sleep(400);
      }
    }, 1200);
    await s.step("Close About fbro", () => i.button("Close"));
    await s.step("Open Settings and Display", async () => {
      await i.command("app.settings");
      await i.button("Display");
      await i.find({ label: "fbro Dark", role: "AXPopUpButton|AXComboBox" });
    }, 1200);
    await s.step("Switch to the light theme", async () => {
      await i.click({ label: "fbro Dark", role: "AXPopUpButton|AXComboBox" });
      await i.key("ArrowDown"); await i.key("Enter");
      await o.wait("light theme", (state) => state.settings.general.colorScheme === "daylight");
    }, 2400);
    await s.step("Switch back to dark mode", async () => {
      await i.click({ label: "fbro Light", role: "AXPopUpButton|AXComboBox" });
      await i.key("ArrowUp"); await i.key("Enter");
      await o.wait("dark theme", (state) => state.settings.general.colorScheme === "fbro-dark");
    }, 1800);
    await s.step("Close Settings", () => i.button("Close"));
    await s.step("Switch to the thumbnail grid", async () => {
      await i.command("panel.toggle_view_mode");
      await o.wait("grid mode", () => o.panel().viewMode === "grid");
      // Thumbnail images have empty alt text and are intentionally omitted
      // from Accessibility. Wait for the grid cards, then let them settle.
      await i.find({ role: "AXRow" }, { panel: "left" });
    }, 2200);
    await s.step("Point out the photo thumbnails", () => i.move(i.stage.x + i.stage.width * 0.3, i.stage.y + i.stage.height * 0.4), 1800);
    await s.step("Return to list mode", async () => { await i.command("panel.toggle_view_mode"); await o.wait("list mode", () => o.panel().viewMode === "list"); });
    await s.step("Filter the photo filenames", async () => {
      await i.command("panel.filter.open"); await i.find({ role: "AXTextField", focused: true });
      // Let each live backend echo settle before the next character. A burst
      // of native Unicode events can otherwise race the controlled input.
      let draft = "";
      for (const character of s.config.filter) {
        await i.type(character); draft += character;
        await o.wait(`filter text ${draft}`, () => o.panel().filter?.value === draft);
        await sleep(100);
      }
      await o.wait("filtered listing", () => o.panel().filter?.value === s.config.filter && o.panel().status === "ready");
    }, 2200);
    await s.step("Clear the filter", async () => { await i.type("", true); await i.key("Escape"); await o.wait("unfiltered listing", () => !o.panel().filter?.value && o.panel().status === "ready"); });
  },
  async "02-collect-shortlist"(s) {
    const { input: i, observer: o, fixture: f } = s;
    await s.step("Create the Shortlist destination", async () => {
      await i.focusPanel("right"); await o.path("right", f.workspace);
      await i.command("file.mkdir"); await i.find({ label: "Name", role: "AXTextField" });
      await i.type("Shortlist"); await i.button("Create");
      await o.wait("new directory", () => o.panel("right").entries.some((entry) => entry.name === "Shortlist"));
      await i.entry("right", "Shortlist"); await i.key("Enter"); await o.path("right", f.shortlist);
    });
    await s.step("Select six original photos", () => s.select(f.photos));
    await s.step("Show the selected photos in grid view", () => s.view("grid"), 2200);
    await s.step("Copy the selected photos into Shortlist", async () => {
      await o.path("left", s.config.pics); await o.selection("left", f.photos); await o.path("right", f.shortlist);
      guardedChild(f.workspace, f.shortlist);
      for (const name of f.photos) assert.equal(fs.existsSync(path.join(f.shortlist, name)), false);
      await i.command("file.copy");
      await i.find({ label: `Copy ${f.photos.length} items to ${f.shortlist}` });
      await s.operation("Copy", async () => { await o.path("right", f.shortlist); await o.selection("left", f.photos); });
      for (const name of f.photos) assert.equal(hashFile(path.join(f.shortlist, name)), hashFile(path.join(s.config.pics, name)));
    }, 1200);
    await s.step("Navigate into the copied shortlist", () => i.go("left", f.shortlist), 1800);
    await s.step("Show the copied photos as a thumbnail grid", () => s.view("grid"), 2400);
    await s.step("Switch the copied photos back to list mode", () => s.view("list"), 1200);
  },
  async "03-remove-rejects"(s) {
    await s.step("Select and delete one rejected photo in grid view", () => s.remove([s.fixture.photos[0]], "grid"), 1500);
    await s.step("Return to list mode for multiple selection", () => s.view("list"));
    await s.step("Select and delete two more rejects together", () => s.remove(s.fixture.photos.slice(1, 3)), 1500);
    assert.deepEqual(fs.readdirSync(s.fixture.shortlist).sort(), s.fixture.photos.slice(3).sort());
    await s.step("Preview the remaining keepers as thumbnails", () => s.view("grid"), 2000);
  },
  async "04-rename-keeper"(s) {
    const { input: i, observer: o, fixture: f } = s;
    const name = f.photos[3];
    await s.step("Choose the cover photo and switch to grid view", async () => {
      await i.entry("left", name); await s.view("grid");
    }, 1800);
    await s.step("Rename the chosen cover photo", async () => {
      await o.selection("left", []); await o.path("left", f.shortlist);
      assert.equal(o.panel().entries[o.panel().cursor]?.name, name);
      guardedChild(f.workspace, path.join(f.shortlist, name));
      await i.command("file.rename"); await i.find({ label: "To", role: "AXTextField" });
      await i.type("cover.jpg", true); await i.button("Rename");
      await o.wait("renamed photo", () => o.panel().entries.some((entry) => entry.name === "cover.jpg"));
    });
    await s.step("Return to list mode to rename the folder", () => s.view("list"));
    await s.step("Rename Shortlist to Presentation", async () => {
      await i.go("left", f.workspace); await i.entry("left", "Shortlist");
      guardedChild(f.workspace, f.shortlist);
      await i.command("file.rename"); await i.find({ label: "To", role: "AXTextField" });
      await i.type("Presentation", true); await i.button("Rename");
      await o.wait("renamed folder", () => o.panel().entries.some((entry) => entry.name === "Presentation"));
      assert.equal(fs.existsSync(path.join(f.workspace, "Presentation/cover.jpg")), true);
    }, 1800);
    await s.step("Preview the renamed Presentation folder in grid view", async () => {
      await i.go("left", path.join(f.workspace, "Presentation")); await s.view("grid");
    }, 2000);
  },
  async "05-markdown-mermaid"(s) {
    const i = s.input;
    await s.step("Open the source architecture document", () => s.viewer(path.basename(s.config.markdown)), 1200);
    await s.step("Show raw Markdown", () => i.key("1"), 1600);
    await s.step("Maximize the Markdown viewer", () => s.maximizeViewer(), 1200);
    await s.step("Search the Markdown source", async () => {
      await i.key("f"); await i.find({ role: "AXTextField", focused: true });
      await i.type("Source"); await i.find({ label: "1 / ", prefix: true });
    }, 1200);
    await s.step("Move to the next search occurrence", async () => { await i.button("Next (Enter)"); await i.find({ label: "2 / ", prefix: true }); }, 1200);
    await s.step("Move forward once more, then to the previous occurrence", async () => {
      await i.button("Next (Enter)"); await i.find({ label: "3 / ", prefix: true });
      await sleep(700); await i.button("Previous (Shift+Enter)"); await i.find({ label: "2 / ", prefix: true });
    }, 1200);
    await s.step("Find the wide source inventory table", async () => {
      await i.click({ role: "AXTextField" });
      await i.type("Local volumes", true); await i.find({ label: "1 / ", prefix: true });
      await i.button("Close (Esc)");
    });
    await s.step("Scroll the Markdown source right and left", async () => {
      await s.keys(Array(6).fill("ArrowRight")); await sleep(800);
      await s.keys(Array(6).fill("ArrowLeft"));
    }, 1000);
    await s.step("Scroll the Markdown source down and up", async () => {
      await s.keys(["ArrowDown", "ArrowDown", "PageDown"]); await sleep(800);
      await s.keys(["PageUp", "ArrowUp", "ArrowUp"]);
    }, 1000);
    await i.key("Home");
    await s.step("Render the Markdown document", () => i.key("4"), 1800);
    await s.step("Scroll the Markdown preview with the keyboard", async () => {
      await s.keys(["ArrowDown", "ArrowDown", "ArrowRight", "ArrowLeft", "ArrowUp", "ArrowUp"]);
    });
    await s.step("Expand the Mermaid diagram", async () => {
      for (let attempt = 0; attempt < 10; attempt++) {
        if (await i.find({ label: "Expand (wheel zoom, drag to pan)", role: "AXButton" }, { optional: true })) break;
        await i.key("PageDown"); await sleep(300);
      }
      await i.button("Expand (wheel zoom, drag to pan)"); await i.button("Fit");
    }, 2500);
    await s.step("Zoom the expanded diagram", () => i.button("Zoom in"), 1800);
    await s.step("Return to the document", () => i.key("Escape"));
    await s.step("Restore the Markdown viewer", () => s.restoreViewer());
    await s.closeViewer();
  },
  async "06-media-viewers"(s) {
    const i = s.input;
    await s.step("Open a photo in the image viewer", async () => { await s.viewer("01-photo.jpg"); await s.imageReady("01-photo.jpg"); }, 1500);
    await s.step("Zoom the photo", () => i.key("z"), 1500);
    await s.step("Move to the next photo", async () => { await i.key("j"); await s.imageReady("02-photo.jpg"); }, 1500);
    await s.closeViewer();
    await s.step("Preview the animated GIF", async () => { await s.viewer("03-animation.gif"); await s.imageReady("03-animation.gif"); }, 2300);
    await s.closeViewer();
    await s.step("Preview the video", async () => { await s.viewer("04-video.mov"); await i.find({ label: "Play", prefix: true, role: "AXButton" }); });
    await s.step("Play the video", () => i.button("Play", { prefix: true }), 3000);
    await s.step("Pause the video", () => i.button("Pause", { prefix: true }), 1200);
    await s.closeViewer();
  },
  async "07-compare-notes"(s) {
    await s.step("Select both versions of the shortlist notes", () => s.select(["shortlist-notes-v1.txt", "shortlist-notes-v2.txt"]));
    await s.step("Compare the two versions", async () => {
      await s.input.command("file.compare");
      await until("comparison window", async () => (await s.input.inspect()).windows.length > 1, s.config.timeoutMs);
      await s.input.contain();
      await s.input.find({ label: "Ready for the presentation.", prefix: true });
    }, 3500);
    await s.closeViewer();
  },
  async "08-explore-git"(s) {
    const i = s.input;
    await s.step("Open the real fbro repository’s Git modal", () => s.gitModal(), 1800);
    await s.step("Browse the commit history", () => i.button("Log", { prefix: true }), 1500);
    await s.step("Expand the latest commit and inspect its first file diff", () => s.historicalDiff(), 3000);
    await s.exploreDiff();
    await s.closeViewer(); await i.key("Escape");
  },
  async "09-edit-review"(s) {
    const { input: i, observer: o, fixture: f } = s;
    await assertDemoClone(f.clone, f.revision);
    await s.step("Open README in embedded Quick Edit", async () => {
      await o.path("left", f.clone);
      await i.entry("left", "README.md");
      await o.path("left", f.clone); assert.equal(o.state.focus, "left");
      await i.command("file.quick_edit");
      await o.wait("embedded Nano terminal", () => o.panel().terminalSessionId != null);
      await i.find({ role: "AXTextArea|AXTextField", focused: true });
    }, 1000);
    await s.step("Append the showcase sentence", async () => {
      await o.path("left", f.clone);
      // Page down to EOF using Nano's portable Ctrl+V binding. This also
      // works with the older Nano supplied by macOS without Option-layout issues.
      for (let page = 0; page < Math.ceil(f.originalReadme.split("\n").length / 10) + 1; page++) await i.key("v", ["ctrl"]);
      await i.key("e", ["ctrl"]);
      // Pico/Nano preserves a final newline when writing the last line.
      await i.type(f.expectedReadme.slice(f.originalReadme.length).replace(/\n$/, ""));
    }, 1000);
    await s.step("Save and close the editor", async () => {
      await i.key("o", ["ctrl"]); await sleep(250); await i.key("Enter"); await sleep(250); await i.key("x", ["ctrl"]);
      await o.wait("Quick Edit completion", () => o.panel().terminalSessionId == null);
      await assertPendingReadme(f);
    });
    await s.step("Review the README change in Git", async () => {
      await s.gitModal(); await i.find({ label: "Select README.md", role: "AXCheckBox" }); await s.diff();
    }, 3000);
    await s.exploreDiff();
    await s.closeViewer(); await i.key("Escape");
  },
  async "10-stage-commit"(s) {
    const { input: i, observer: o, fixture: f } = s;
    await o.path("left", f.clone);
    await assertPendingReadme(f);
    await s.step("Open Git changes in the independent demo clone", () => s.gitModal(), 1500);
    await s.step("Stage only README.md", async () => {
      await o.path("left", f.clone); assert.equal(o.state.focus, "left");
      await assertPendingReadme(f);
      await i.button("Stage");
      await until("README staged", async () => (await git(f.clone, ["diff", "--cached", "--name-only"])) === "README.md", s.config.timeoutMs);
      await assertPendingReadme(f, { staged: true });
    }, 1500);
    await s.step("Write the commit message", async () => { await i.click({ role: "AXTextArea" }); await i.type(commitMessage); });
    await s.step("Commit the staged documentation change", async () => {
      await o.path("left", f.clone); assert.equal(o.state.focus, "left");
      await assertPendingReadme(f, { staged: true });
      await i.button("Commit staged", { prefix: true });
      await until("new demo commit", async () => (await git(f.clone, ["rev-parse", "HEAD"])) !== f.revision, s.config.timeoutMs);
      s.clip.commitHash = await verifyDemoCommit(f);
    }, 1500);
    await s.step("Inspect the new commit and its README diff", async () => {
      await i.button("Log", { prefix: true });
      await i.find({ label: commitMessage, prefix: true });
      await s.historicalDiff();
    }, 3000);
    await s.exploreDiff();
    await s.closeViewer(); await i.key("Escape");
  },
};

export async function playScene(scene) {
  scene.startedAt ??= performance.now();
  await sleep(2000 * scene.config.pace);
  await flows[scene.fixture.id](scene);
  await sleep(2000 * scene.config.pace);
}
