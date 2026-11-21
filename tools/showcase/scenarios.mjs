import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import { commitMessage, git, guardedChild, hashFile, repoRoot, showcaseZoomSteps, sleep, until } from "./core.mjs";
import { assertDemoClone, assertPendingReadme, verifyDemoCommit } from "./fixtures.mjs";

export const chapters = [
  ["01-browse-filter", "Customize, browse, and filter", "Start with the demo photo grid, show quick preview on the right while selecting several photos, then open About through the command palette, point at the user guide, and continue with zoom, themes, list view, and filtering."],
  ["02-collect-shortlist", "Collect a photo shortlist", "Create a destination folder, select six photos, copy, navigate into the result, and switch between grid and list."],
  ["03-remove-rejects", "Remove rejected photos", "Delete one copied photo in grid mode, return to list to delete two together, and preview the remaining thumbnails."],
  ["04-rename-keeper", "Rename the keepers", "Rename a photo in grid mode, return to list to rename its folder, then preview the renamed collection."],
  ["05-markdown-mermaid", "Read Markdown and Mermaid", "Maximize the Markdown viewer, search and navigate occurrences, scroll with the keyboard in four directions, then render Markdown and expand a Mermaid diagram."],
  ["06-media-viewers", "Preview photos and media", "JPEG zoom and next-file navigation, animated GIF, and MOV playback."],
  ["07-compare-notes", "Compare and inspect shortlist notes", "Compare two text versions, then search for the notes and press i on a result to inspect its real file information."],
  ["08-explore-git", "Explore fbro’s Git history", "Inspect status, history, and a historical diff. Maximize briefly for a single change; navigate and switch views only for multiple change blocks."],
  ["09-edit-review", "Edit and review documentation", "Edit README and review its diff, then run ls -al and mc in the embedded terminal, maximize and promote it to a tab, and return to files."],
  ["10-stage-commit", "Stage and commit", "Stage only README, commit, and inspect the new commit with a brief maximized diff."],
  ["11-select-by-mask", "Select files by mask", "Select every JPEG with the default mask, then deselect the photo-* names and leave the remaining slide selected."],
  ["12-keyboard", "Button bar, cheat sheet, and shortcuts", "Hide and restore the button bar, page the cheat sheet, open a shortcut without saving it, then switch the bar to function keys and view a photo with F3."],
  ["13-color-schemes", "Bundled and custom color schemes", "Preview Matrix, duplicate it as Showcase, recolor the header, and apply that scheme to the photo grid."],
  ["14-editor-tabs", "Editor tabs and notes", "Edit a checksum file, a script, and Markdown in editor tabs, toggle line numbers, whitespace, and wrapping, preview the note, and write a new note in the demo folder."],
  ["15-panel-tabs", "Panel tabs", "Open a tab per folder, drag one in front of another, cycle with Ctrl+Tab, jump through the tab list, and close the others from the tab menu."],
  ["16-permissions", "Set permissions", "Set a script to mode 755 from the octal field and confirm the listing and Info show rwxr-xr-x."],
  ["17-job-files", "Job file list", "Copy two notes into Incoming, leave that folder, then jump from the finished job’s file list back to the source."],
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
    "11-select-by-mask": fixture.workspace,
    "12-keyboard": fixture.workspace,
    "13-color-schemes": fixture.workspace,
    "14-editor-tabs": fixture.workspace,
    "15-panel-tabs": fixture.album,
    "16-permissions": fixture.workspace,
    "17-job-files": fixture.workspace,
  }[fixture.id];
  const ownWorkspace = ["02-collect-shortlist", "04-rename-keeper", "11-select-by-mask", "12-keyboard", "13-color-schemes", "14-editor-tabs", "16-permissions", "17-job-files"];
  const right = fixture.id === "15-panel-tabs" ? fixture.album
    : ownWorkspace.includes(fixture.id) ? fixture.workspace
    : ["09-edit-review", "10-stage-commit", "01-browse-filter"].includes(fixture.id) ? repoRoot : config.pics;
  if (!left || !right) throw new Error(`No starting paths for ${fixture.id}`);
  return { left, right };
}

const DARK_SCHEME = "fbro Dark (built-in)";
const LIGHT_SCHEME = "fbro Light (built-in)";
const NUMBER_KEYS = "Number keys 1\u20130";
const FUNCTION_KEYS = "Function keys F1\u2013F10";

function commandKey(state, id) {
  return state.commands?.find((item) => item.id === id)?.bindings?.[0]?.strokes?.[0]?.key;
}

function schemeItem(state, id) {
  return state.settings?.general?.colorSchemes?.items?.find((item) => item.id === id);
}

function tabPaths(state, slot = "left") {
  return (state.slots?.[slot]?.tabPanelIds ?? []).map((id) => state.panels[id]?.path ?? "");
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

  async viewer(name, { open = "command" } = {}) {
    await this.input.entry("left", name);
    const state = await this.input.inspect();
    if (state.windows.length !== 1 || state.windows[state.focusedWindow]?.title !== this.input.mainTitle) {
      throw new Error("Viewer opening requires the main listing; another window is already open");
    }
    // The toolbar button's children collapse into one control named "F3 View".
    // Clicking it runs View without sending F3, which macOS may keep for Mission Control.
    if (open === "button") await this.input.click({ label: "F3 View", role: "AXButton" });
    else await this.input.command("file.view");
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
    let count;
    await until("comparison change-block count", async () => {
      const state = await this.input.inspect();
      const counter = state.nodes.find(node => node.window === state.focusedWindow && node.role === "AXStaticText" && /^\d+\/\d+$/.test(node.value));
      if (!counter) return false;
      count = Number(counter.value.split("/")[1]);
      return true;
    }, this.config.timeoutMs);
    this.clip.diffChangeBlocks = count;
    await this.step("Maximize the comparison", () => this.maximizeViewer(), count <= 1 ? 1800 : 1000);
    if (count <= 1) return;
    await this.step("Navigate down through the diff", () => this.keys(["ArrowDown", "PageDown"]), 1200);
    await this.step("Navigate back up through the diff", () => this.keys(["PageUp", "ArrowUp"]), 1200);
    await this.step("Switch to unified diff mode", () => this.input.button("Unified view"), 1600);
    await this.step("Scroll down and up in unified mode", () => this.keys(["PageDown", "PageUp"]), 1000);
    await this.step("Return to side-by-side mode", () => this.input.button("Side by side view"), 1200);
    await this.step("Restore the comparison window", () => this.restoreViewer());
  }

  async terminalTour(directory) {
    const { input: i, observer: o } = this;
    let sessionId, terminalPanelId;
    await this.step("Open the embedded terminal in the edited repository", async () => {
      await o.path("left", directory);
      await i.focusPanel("left");
      await i.command("panel.toggle_embedded_terminal");
      await o.wait("embedded shell session", () => o.panel("left").terminalSessionId != null);
      sessionId = o.panel("left").terminalSessionId;
      await i.find({ role: "AXTextArea|AXTextField", focused: true });
    }, 1200);
    await this.step("List the real folder contents with ls -al", async () => {
      await i.type("ls -al"); await i.key("Enter");
    }, 2000);
    await this.step("Launch Midnight Commander with mc", async () => {
      await i.type("mc"); await i.key("Enter");
    }, 2500);
    await this.step("Expand the terminal to fill the app", async () => {
      // Maximize reveals the toolbar, then hides it after 2.5s. Hidden buttons
      // use visibility:hidden, so they leave the accessibility tree, and a full
      // scan is slower than that grace. Park the pointer on the top-right chip
      // once the fixed layout has moved it under the title bar; hover keeps it up.
      await i.command("panel.terminal.maximize");
      await sleep(400);
      const x = i.stage.x + i.stage.width - 36;
      const y = i.stage.y + 46;
      await i.move(x, y);
      await sleep(200);
      await i.move(x, y);
      // aria-pressed exposes the maximized control as a checkbox, not a button.
      await i.find({ label: "Restore terminal", role: "AXButton|AXCheckBox" });
    }, 2000);
    await this.step("Promote the live terminal to its own tab", async () => {
      await i.command("panel.terminal.promote");
      await o.wait("terminal promoted with the same live session", state => {
        terminalPanelId = state.slots.left.activePanelId;
        const terminal = state.panels[terminalPanelId];
        return terminalPanelId !== "left" && terminal?.tabKind === "terminal"
          && terminal.terminalSessionId === sessionId && o.panel("left").terminalSessionId == null;
      });
    }, 1800);
    await this.step("Return to the previous files tab and keep the terminal running", async () => {
      // Both tabs keep the folder name. The original files caption comes first
      // and has no terminal icon; clicking its text avoids the tab-close button.
      await i.click({ label: path.basename(directory), role: "AXStaticText" }, { panel: "left" });
      await o.wait("original files tab restored", state => state.slots.left.activePanelId === "left" && state.focus === "left"
        && state.panels[terminalPanelId]?.terminalSessionId === sessionId);
      await o.path("left", directory);
      assert.equal(o.panel("left").tabKind, "listing");
    }, 1200);
  }

  async showToolbarKey(key) {
    // WebKit exposes the key, icon, and label as one button, named "3 View"
    // or "F3 View". The glyph sits left of the centered cluster.
    const button = await this.input.find({ label: `${key} View`, role: "AXButton" });
    const { x, y, width, height } = button.frame;
    const midY = y + height / 2;
    await this.input.move(x + width * 0.32, midY);
    await sleep(200);
    await this.input.move(x + width * 0.66, midY);
  }

  async focusSlot(slot = "left") {
    await this.observer.refresh();
    const activeId = this.observer.slot(slot).activePanelId;
    if (this.observer.state.focus === activeId || this.observer.state.focus === slot) return;
    await this.input.move(this.input.stage.x + this.input.stage.width * (slot === "left" ? 0.23 : 0.73), this.input.stage.y + this.input.stage.height * 0.55, true);
    await this.observer.wait(`${slot} slot focused`, (state) => {
      const current = state.slots?.[slot]?.activePanelId;
      return state.focus === current || state.focus === slot;
    });
  }

  async entryActive(name, slot = "left") {
    await this.focusSlot(slot);
    const panelId = this.observer.slot(slot).activePanelId;
    const panel = this.observer.panel(panelId);
    if (panel.tabKind !== "listing") throw new Error(`Active ${slot} tab is not a listing`);
    await this.input.entry(panelId, name);
  }

  async goActive(destination, slot = "left") {
    await this.focusSlot(slot);
    if (this.observer.active(slot).tabKind !== "listing") throw new Error(`Active ${slot} tab is not a listing`);
    await this.input.command("panel.navigate_to_path");
    await this.input.find({ role: "AXTextField", focused: true });
    await this.input.type(destination, true);
    await this.input.key("Enter");
    await this.observer.wait(`${slot} active tab at ${destination}`, () => {
      const panel = this.observer.active(slot);
      return panel.tabKind === "listing" && panel.path === destination && panel.status === "ready";
    });
    await sleep(200);
  }

  async openEditor(name, slot = "left") {
    await this.entryActive(name, slot);
    await this.input.click({ label: `Edit ${name} in editor tab`, role: "AXButton" }, { panel: slot });
    await this.observer.wait(`editor tab for ${name}`, () => {
      const active = this.observer.active(slot);
      return active.tabKind === "editor" && String(active.editor?.filePath ?? "").endsWith(`/${name}`);
    });
    await this.input.find({ role: "AXTextArea|AXTextField", focused: true });
  }

  async leaveEditor(slot = "left") {
    const origin = this.observer.active(slot).editor?.originPanelId;
    await this.input.key("Escape");
    await this.observer.wait("origin listing restored", () => {
      const active = this.observer.active(slot);
      const activeId = this.observer.slot(slot).activePanelId;
      return active.tabKind === "listing" && (!origin || activeId === origin);
    });
  }

  async clickEditor() {
    await this.input.click({ role: "AXTextArea|AXTextField" });
    await this.input.find({ role: "AXTextArea|AXTextField", focused: true });
  }

  async toggleEditor(label, field, expected) {
    await this.input.button(label);
    await this.observer.wait(`${label} ${expected}`, (state) => state.settings.general[field] === expected);
  }

  async choosePopup(label, downs) {
    await this.input.click({ label, role: "AXPopUpButton|AXComboBox" });
    for (let step = 0; step < downs; step++) await this.input.key("ArrowDown");
    await this.input.key("Enter");
  }

  center(node) {
    return { x: node.frame.x + node.frame.width / 2, y: node.frame.y + node.frame.height / 2 };
  }
}

const flows = {
  async "01-browse-filter"(s) {
    const { input: i, observer: o } = s;
    const photos = s.fixture.previewPhotos;
    const filter = s.fixture.filter;
    await s.step("Start with the entire photos folder in grid view", async () => {
      await o.path("left", s.config.pics);
      await o.wait("opening photo grid", () => o.panel("left").viewMode === "grid");
      const target = await i.find({ label: photos[0], role: "AXStaticText" }, { panel: "left" });
      await i.move(target.frame.x + target.frame.width / 2, target.frame.y + target.frame.height / 2);
    }, 1000);
    await s.step("Open the larger right-hand quick preview with Cmd+3", async () => {
      await i.command("panel.viewer.embed");
      await i.find({ label: photos[0], role: "AXImage" }, { panel: "right" });
    }, 1500);
    for (const name of photos.slice(1)) {
      await s.step(`Move the file cursor to ${name} and update the right preview`, async () => {
        await i.click({ label: name, role: "AXStaticText" }, { panel: "left" });
        await o.wait(`grid cursor ${name}`, () => { const panel = o.panel("left"); return panel.entries[panel.cursor]?.name === name; });
        await i.find({ label: name, role: "AXImage" }, { panel: "right" });
      }, 1500);
    }
    await s.step("Close quick preview and return to both file panels", async () => {
      await i.command("panel.viewer.embed");
      await until("right preview closed", async () => !(await i.find({ label: photos.at(-1), role: "AXImage" }, { optional: true, panel: "right" })), s.config.timeoutMs);
    }, 600);
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
    await s.step("Point at the user guide without opening it", async () => {
      const guide = await i.find({ label: "User guide", role: "AXButton" });
      const point = s.center(guide);
      await i.move(point.x, point.y);
    }, 1200);
    await s.step("Close About fbro", () => i.button("Close"));
    await s.step("Open Settings and Display", async () => {
      await i.command("app.settings");
      await i.button("Display");
      await i.find({ label: DARK_SCHEME, role: "AXPopUpButton|AXComboBox" });
    }, 1200);
    await s.step("Switch to the light theme", async () => {
      await i.click({ label: DARK_SCHEME, role: "AXPopUpButton|AXComboBox" });
      await i.key("ArrowDown"); await i.key("Enter");
      await o.wait("light theme", (state) => state.settings.general.colorScheme === "daylight");
    }, 2400);
    await s.step("Switch back to dark mode", async () => {
      await i.click({ label: LIGHT_SCHEME, role: "AXPopUpButton|AXComboBox" });
      await i.key("ArrowUp"); await i.key("Enter");
      await o.wait("dark theme", (state) => state.settings.general.colorScheme === "fbro-dark");
    }, 1800);
    await s.step("Close Settings", () => i.button("Close"));
    await s.step("Return to list mode", () => s.view("list"));
    await s.step("Filter the photo filenames", async () => {
      await i.command("panel.filter.open"); await i.find({ role: "AXTextField", focused: true });
      // Let each live backend echo settle before the next character. A burst
      // of native Unicode events can otherwise race the controlled input.
      let draft = "";
      for (const character of filter) {
        await i.type(character); draft += character;
        await o.wait(`filter text ${draft}`, () => o.panel().filter?.value === draft);
        await sleep(100);
      }
      await o.wait("filtered listing", () => o.panel().filter?.value === filter && o.panel().status === "ready");
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
    const { input: i, observer: o, fixture: f } = s;
    await s.step("Select both versions of the shortlist notes", () => s.select(["shortlist-notes-v1.txt", "shortlist-notes-v2.txt"]));
    await s.step("Compare the two versions", async () => {
      await s.input.command("file.compare");
      await until("comparison window", async () => (await s.input.inspect()).windows.length > 1, s.config.timeoutMs);
      await s.input.contain();
      await s.input.find({ label: "Ready for the presentation.", prefix: true });
    }, 600);
    await s.exploreDiff();
    await s.closeViewer();
    await s.step("Search the demo folder for shortlist notes", async () => {
      await i.focusPanel("left");
      await i.command("panel.selection.clear");
      await o.selection("left", []);
      await i.command("app.search");
      await i.click({ label: "File name", role: "AXTextField" });
      await i.type("shortlist-notes-*.txt", true);
      await i.button("Search");
      await i.find({ label: "To Results", role: "AXButton" });
    }, 1200);
    await s.step("Show the matching files in the search results listing", async () => {
      await i.button("To Results");
      await o.wait("shortlist notes search results mounted", () => {
        const panel = o.panel("left");
        return panel.status === "ready" && panel.sourceId?.startsWith("search:")
          && ["shortlist-notes-v1.txt", "shortlist-notes-v2.txt"].every(name => panel.entries.some(entry => entry.name === name));
      });
      await i.entry("left", "shortlist-notes-v2.txt");
      const panel = o.panel("left");
      assert.equal(panel.entries[panel.cursor].sourcePath, path.join(f.workspace, "shortlist-notes-v2.txt"));
    }, 1000);
    await s.step("Press i to inspect a search result’s real file information", async () => {
      await i.command("file.info");
      await i.find({ label: "Info" });
      await i.find({ label: path.join(f.workspace, "shortlist-notes-v2.txt") });
      await i.find({ label: "Modified" });
    }, 2500);
    await s.step("Close Info and return to the search result", async () => {
      await i.key("Escape");
      await i.entry("left", "shortlist-notes-v2.txt");
    });
  },
  async "08-explore-git"(s) {
    const i = s.input;
    await s.step("Open the real fbro repository’s Git modal", () => s.gitModal(), 1800);
    await s.step("Browse the commit history", () => i.button("Log", { prefix: true }), 1500);
    await s.step("Expand the latest commit and inspect its first file diff", () => s.historicalDiff(), 600);
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
    await s.step("Find the Design docs section in the middle of README", async () => {
      await o.path("left", f.clone);
      // Ctrl+W search works in both GNU Nano and macOS Pico. Insert above
      // the heading, leaving existing text and the file's final newline intact.
      await i.key("w", ["ctrl"]); await i.type("## Design docs"); await i.key("Enter");
      await i.key("a", ["ctrl"]);
    }, 800);
    await s.step("Insert the showcase paragraph above Design docs", async () => {
      await i.type(f.readmeInsertText);
    }, 1000);
    await s.step("Save and close the editor", async () => {
      await i.key("o", ["ctrl"]); await sleep(250); await i.key("Enter"); await sleep(250); await i.key("x", ["ctrl"]);
      await o.wait("Quick Edit completion", () => o.panel().terminalSessionId == null);
      await assertPendingReadme(f);
    });
    await s.step("Review the README change in Git", async () => {
      await s.gitModal(); await i.find({ label: "Select README.md", role: "AXCheckBox" }); await s.diff();
    }, 600);
    await s.exploreDiff();
    await s.closeViewer(); await i.key("Escape");
    await s.terminalTour(f.clone);
    await assertPendingReadme(f);
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
    }, 600);
    await s.exploreDiff();
    await s.closeViewer(); await i.key("Escape");
  },
  async "11-select-by-mask"(s) {
    const { input: i, observer: o, fixture: f } = s;
    await s.step("Put the cursor on a JPEG so the mask starts at *.jpg", () => i.entry("left", "photo-a.jpg"));
    await s.step("Select every JPEG with the default mask", async () => {
      await i.command("panel.selection.select_by_mask");
      await i.find({ label: "Select Files" });
      await i.find({ label: "*.jpg", role: "AXTextField|AXComboBox" });
      await i.button("Select");
      await o.selection("left", f.maskPhotos);
    }, 1400);
    await s.step("Deselect the photo-* names and keep the slide", async () => {
      await i.command("panel.selection.deselect_by_mask");
      await i.find({ label: "Deselect Files" });
      await i.find({ role: "AXTextField|AXComboBox", focused: true });
      await i.type("photo-*.jpg", true);
      await i.button("Deselect");
      await o.selection("left", ["slide-c.jpg"]);
    }, 1600);
  },
  async "12-keyboard"(s) {
    const { input: i, observer: o } = s;
    await s.step("Show the number-key button bar", async () => {
      await o.wait("number-key View binding", (state) => state.settings.general.showButtonBar && commandKey(state, "file.view") === "3");
      await s.showToolbarKey("3");
    }, 1400);
    await s.step("Hide the button bar from Display settings", async () => {
      await i.command("app.settings");
      await i.button("Display");
      await i.click({ label: "Show bottom button bar", role: "AXCheckBox|AXStaticText" });
      await o.wait("button bar hidden", (state) => state.settings.general.showButtonBar === false);
      await i.button("Close");
      await until("button bar removed", async () => !(await i.find({ label: "3 View", role: "AXButton" }, { optional: true })), s.config.timeoutMs);
    }, 1200);
    await s.step("Restore the button bar from the command palette", async () => {
      await i.command("app.toggle_button_bar");
      await o.wait("button bar restored", (state) => state.settings.general.showButtonBar === true && commandKey(state, "file.view") === "3");
      await i.find({ label: "3 View", role: "AXButton" });
    }, 1400);
    await s.step("Open the keyboard cheat sheet and page through it", async () => {
      await i.command("app.cheat_sheet");
      await i.find({ label: "Keyboard Cheat Sheet" });
      await i.key("PageDown");
    }, 1400);
    await s.step("Open shortcut settings from the cheat sheet", async () => {
      await i.button("Customize…");
      await i.find({ label: "Search commands", role: "AXTextField|AXComboBox" });
    }, 1000);
    await s.step("Show Inspect Item’s shortcut without saving a change", async () => {
      await i.click({ label: "Search commands", role: "AXTextField|AXComboBox" });
      await i.type("Inspect Item");
      await i.click({ label: "Inspect Item", prefix: true, role: "AXStaticText|AXButton" }, { aboveToolbar: true });
      // The listing's inline Edit tooltip is also "Edit", and it is the smaller node.
      await i.button("Edit", { aboveToolbar: true, titleOnly: true });
      await i.find({ label: "Edit shortcut" });
      await i.button("Cancel", { aboveToolbar: true });
    }, 1400);
    await s.step("Switch the button bar from number keys to function keys", async () => {
      if (o.state.settings?.general?.systemFunctionKeysStandard == null) throw new Error("Button bar key choice is only available when macOS reports the function-key setting");
      await s.choosePopup(NUMBER_KEYS, 1);
      await o.wait("function-key View binding", (state) => state.settings.general.buttonBarKeys === "function" && commandKey(state, "file.view") === "F3");
      await i.find({ label: FUNCTION_KEYS, role: "AXPopUpButton|AXComboBox" });
    }, 1400);
    await s.step("Close settings and show F3 on the button bar", async () => {
      await i.button("Close");
      await s.showToolbarKey("F3");
    }, 1000);
    await s.step("View the photo with F3", async () => {
      // A Mac that still maps F3 to Mission Control would take the key before
      // fbro. The button already shows F3, so open the photo from that control.
      const standard = o.state.settings?.general?.systemFunctionKeysStandard === true;
      await s.viewer("view.jpg", { open: standard ? "command" : "button" });
      await s.imageReady("view.jpg");
    }, 1600);
    await s.closeViewer();
  },
  async "13-color-schemes"(s) {
    const { input: i, observer: o, fixture: f } = s;
    await s.step("Open the color scheme editor", async () => {
      await o.path("left", f.workspace);
      await i.command("app.settings");
      await i.button("Display");
      await i.button("Customize…");
      await i.find({ label: DARK_SCHEME, role: "AXPopUpButton|AXComboBox" });
    }, 1200);
    await s.step("Preview the bundled Matrix scheme", async () => {
      // Built-ins, then bundled sources: Dark, Light, CDE Motif, IntelliJ Dark,
      // IntelliJ Light, Magenta, Matrix. Six downs stays valid while that order does.
      await s.choosePopup(DARK_SCHEME, 6);
      await i.find({ label: "Matrix (built-in)", role: "AXPopUpButton|AXComboBox" });
      await i.button("Use this scheme");
      await o.wait("Matrix in use", (state) => state.settings.general.colorScheme === "bundled:matrix");
    }, 1800);
    await s.step("Duplicate Matrix as Showcase", async () => {
      await i.button("Duplicate…");
      await i.find({ role: "AXTextField|AXComboBox", focused: true });
      await i.type("Showcase", true);
      await i.button("Create");
      await o.wait("Showcase scheme saved", (state) => schemeItem(state, "custom:showcase"));
      await i.find({ label: "Showcase (custom)", role: "AXPopUpButton|AXComboBox" });
    }, 1400);
    await s.step("Recolor the header purple", async () => {
      await i.click({ label: "Filter colors", role: "AXTextField|AXComboBox" });
      await i.type("Header");
      // The row's label and hex are one accessible name, "Header #04180a".
      await i.click({ label: "Header", role: "AXStaticText", prefix: true }, { aboveToolbar: true });
      // The hex field is below the window until focus scrolls the settings pane.
      // The color picker has three sliders before that field.
      let field;
      for (let attempt = 0; attempt < 6 && !field; attempt++) {
        await i.key("Tab");
        await sleep(200);
        field = await i.find({ label: "Header value", role: "AXTextField|AXComboBox", focused: true }, { optional: true });
      }
      if (!field) await i.find({ label: "Header value", role: "AXTextField|AXComboBox", focused: true });
      await i.type("#6d28d9", true);
      await o.wait("header color saved", (state) => schemeItem(state, "custom:showcase")?.colors?.["bg-header"]?.toLowerCase() === "#6d28d9");
      await i.find({ label: "Saved" });
    }, 1400);
    await s.step("Use the Showcase scheme and close settings", async () => {
      // The hex field scrolls this button above the pane. Its accessibility
      // frame then sits on the right panel's path bar, so a click opens that
      // folder instead. Shift+Tab brings the button back and keys activate it.
      let use;
      for (let attempt = 0; attempt < 16 && !use; attempt++) {
        await i.key("Tab", ["shift"]);
        await sleep(150);
        use = await i.find({ label: "Use this scheme", role: "AXButton", focused: true }, { optional: true });
      }
      if (!use) await i.find({ label: "Use this scheme", role: "AXButton", focused: true });
      await i.key("Enter");
      await o.wait("Showcase scheme in use", (state) => state.settings.general.colorScheme === "custom:showcase");
      await i.button("Close");
    }, 800);
    await s.step("Show the recolored panels as a photo grid", () => s.view("grid"), 2200);
  },
  async "14-editor-tabs"(s) {
    const { input: i, observer: o, fixture: f } = s;
    const script = path.join(f.workspace, "shortlist.js");
    await s.step("Open the checksum file with its inline Edit button", () => s.openEditor("SHA256SUMS"), 1600);
    await s.step("Return to the listing and keep the editor tab", () => s.leaveEditor(), 1000);
    await s.step("Open the shortlist script in another editor tab", () => s.openEditor("shortlist.js"), 1200);
    await s.step("Show line numbers and whitespace, then wrap the long line", async () => {
      await s.toggleEditor("Show line numbers", "editorLineNumbers", true);
      await s.toggleEditor("Show whitespace", "editorShowWhitespace", true);
      await s.toggleEditor("Wrap long lines", "editorWordWrap", false);
      await sleep(700);
      await s.toggleEditor("Wrap long lines", "editorWordWrap", true);
    }, 1400);
    await s.step("Append a comment and save it", async () => {
      await s.clickEditor();
      await i.key("ArrowDown", ["cmd"]);
      await i.type(`\n${f.editorMark}`);
      await i.key("s", ["cmd"]);
      await until("saved showcase comment", () => fs.readFileSync(script, "utf8").includes(f.editorMark), s.config.timeoutMs);
    }, 1200);
    await s.step("Open the editor shortcut sheet", async () => {
      await i.button("Editor shortcuts");
      await i.find({ label: "Editor Shortcuts" });
    }, 1600);
    await s.step("Close the shortcut sheet", () => i.button("Close"), 600);
    await s.step("Return to the listing", () => s.leaveEditor(), 800);
    await s.step("Preview the Markdown note beside the editor", async () => {
      await s.openEditor("showcase.md");
      await i.command("panel.viewer.embed");
      await i.find({ label: "Showcase notes" }, { panel: "right" });
    }, 1800);
    await s.step("Close the preview and return to the listing", async () => {
      await i.command("panel.viewer.embed");
      await until("markdown preview closed", async () => !(await i.find({ label: "Showcase notes" }, { optional: true, panel: "right" })), s.config.timeoutMs);
      await s.leaveEditor();
    }, 800);
    await s.step("Create a note in the demo folder and save a sentence", async () => {
      await i.command("notes.new");
      await o.wait("new note editor", () => {
        const active = o.active("left");
        return active.tabKind === "editor" && active.editor?.isNote === true;
      });
      await s.clickEditor();
      await i.type(f.noteMark);
      await i.key("s", ["cmd"]);
      await until("saved demo note", () => fs.readdirSync(f.workspace).some((name) => name.endsWith(".md") && name !== "showcase.md" && fs.readFileSync(path.join(f.workspace, name), "utf8").includes(f.noteMark)), s.config.timeoutMs);
    }, 1600);
  },
  async "15-panel-tabs"(s) {
    const { input: i, observer: o, fixture: f } = s;
    for (const name of f.folders) {
      await s.step(`Open a tab for ${name}`, async () => {
        await s.focusSlot("left");
        const before = o.slot("left").activePanelId;
        const count = o.slot("left").tabPanelIds.length;
        await i.command("panel.tab.new");
        await o.wait(`new tab after ${before}`, (state) => state.slots.left.tabPanelIds.length === count + 1 && state.slots.left.activePanelId !== before);
        await s.goActive(path.join(f.album, name));
      }, 900);
    }
    await s.step("Drag Charlie in front of Alpha", async () => {
      // Short names stay within the tab label limit and on screen together,
      // so the drag does not depend on the strip scrolling.
      await o.wait("Delta tab active", () => o.active("left").path === path.join(f.album, "Delta"));
      const from = await i.find({ label: "Charlie", role: "AXStaticText" }, { panel: "left" });
      const to = await i.find({ label: "Alpha", role: "AXStaticText" }, { panel: "left" });
      const start = s.center(from);
      await i.drag(start.x, start.y, to.frame.x + to.frame.width * 0.25, to.frame.y + to.frame.height / 2);
      await o.wait("Charlie before Alpha", (state) => {
        const paths = tabPaths(state);
        const charlie = paths.findIndex((item) => item.endsWith("/Charlie"));
        const alpha = paths.findIndex((item) => item.endsWith("/Alpha"));
        return charlie >= 0 && charlie === alpha - 1;
      });
    }, 1400);
    await s.step("Cycle to the next tab with Ctrl+Tab", async () => {
      const before = o.slot("left").activePanelId;
      await i.command("panel.tab.next");
      await o.wait("next tab active", (state) => state.slots.left.activePanelId !== before && state.panels[state.slots.left.activePanelId]?.path === f.album);
    }, 1200);
    await s.step("Jump to Bravo from the tab list", async () => {
      await i.command("panel.tab.list");
      await i.click({ label: "Search tabs", role: "AXTextField|AXComboBox" });
      await i.type("Bravo");
      await i.key("Enter");
      await o.wait("Bravo tab active", () => o.active("left").path === path.join(f.album, "Bravo"));
      await until("tab list closed", async () => !(await i.find({ label: "Search tabs" }, { optional: true })), s.config.timeoutMs);
    }, 1200);
    await s.step("Close the other tabs from Alpha’s menu", async () => {
      await i.rightClick({ label: "Alpha", role: "AXStaticText" }, { panel: "left" });
      await i.click({ label: "Close others", role: "AXMenuItem|AXButton|AXStaticText" });
      await o.wait("only the Alpha tab remains", (state) => {
        const paths = tabPaths(state);
        return paths.length === 1 && paths[0] === path.join(f.album, "Alpha");
      });
    }, 1400);
  },
  async "16-permissions"(s) {
    const { input: i, observer: o, fixture: f } = s;
    const script = path.join(f.workspace, "present.sh");
    await s.step("Open Set Permissions for the script", async () => {
      await i.entry("left", "present.sh");
      assert.equal(fs.statSync(script).mode & 0o777, 0o644);
      await i.command("file.set_permissions");
      await i.find({ label: "Set Permissions" });
    }, 1000);
    await s.step("Set the octal mode to 755 and apply it", async () => {
      await i.click({ role: "AXTextField" });
      await i.type("755", true);
      await i.button("Apply");
      await until("permissions dialog closed", async () => !(await i.find({ label: "Set Permissions" }, { optional: true })), s.config.timeoutMs);
      await o.wait("listing shows rwxr-xr-x", () => o.panel("left").entries.find((entry) => entry.name === "present.sh")?.permissions === "rwxr-xr-x");
      assert.equal(fs.statSync(script).mode & 0o777, 0o755);
    }, 1200);
    await s.step("Confirm the mode in Info", async () => {
      await i.command("file.info");
      await i.find({ label: "Permissions" });
      await i.find({ label: "rwxr-xr-x" });
    }, 1800);
    await s.step("Close Info", () => i.key("Escape"));
  },
  async "17-job-files"(s) {
    const { input: i, observer: o, fixture: f } = s;
    await s.step("Create Incoming on the right and open it", async () => {
      await i.focusPanel("right");
      await o.path("right", f.workspace);
      await i.command("file.mkdir");
      await i.find({ label: "Name", role: "AXTextField" });
      await i.type("Incoming");
      await i.button("Create");
      await o.wait("Incoming directory", () => o.panel("right").entries.some((entry) => entry.name === "Incoming"));
      await i.entry("right", "Incoming");
      await i.key("Enter");
      await o.path("right", f.incoming);
    }, 1000);
    await s.step("Copy the two notes into Incoming", async () => {
      await s.select(f.copied, "left");
      await o.path("left", f.workspace);
      await o.path("right", f.incoming);
      for (const name of f.copied) assert.equal(fs.existsSync(path.join(f.incoming, name)), false);
      await i.command("file.copy");
      await i.find({ label: `Copy ${f.copied.length} items to ${f.incoming}` });
      await s.operation("Copy", async () => { await o.selection("left", f.copied); await o.path("right", f.incoming); });
      for (const name of f.copied) assert.equal(fs.existsSync(path.join(f.incoming, name)), true);
    }, 1000);
    await s.step("Leave the source folder so the jump is visible", () => i.go("left", f.incoming), 1000);
    await s.step("Expand the finished copy and jump back to a source file", async () => {
      await o.path("left", f.incoming);
      assert.equal(o.state.focus, "left");
      await i.command("app.open_jobs");
      // The section title is uppercased by CSS, and that is the accessible name.
      await i.find({ label: "FINISHED FILE JOBS" });
      await i.button("Expand File List");
      const row = await i.find({ label: "File 1 of 2:", prefix: true });
      const point = s.center(row);
      await i.move(point.x, point.y);
      await i.button("Jump to location");
      await o.wait("jumped back to the source folder", () => o.panel("left").path === f.workspace && o.panel("left").status === "ready" && o.state.focus === "left");
    }, 1600);
  },
};

export async function playScene(scene) {
  scene.startedAt ??= performance.now();
  await sleep(2000 * scene.config.pace);
  await flows[scene.fixture.id](scene);
  await sleep(2000 * scene.config.pace);
}
