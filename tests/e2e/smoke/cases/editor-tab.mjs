import fs from "node:fs";
import path from "node:path";

import { clickContextMenuItem, clickEntry, goToPanelPath, openPanelContextMenu } from "../browser.mjs";
import { artifactsDir, backendPath, hostPath } from "../config.mjs";
import { sleep, waitFor } from "../wait.mjs";

const KEY = {
  ctrl: "",
  shift: "",
  escape: "",
  end: "",
  home: "",
  f3: "",
  f4: "",
  tab: "",
};

/** Real key actions: CodeMirror ignores synthetic keydown events for text. */
async function chord(browser, modifiers, key) {
  await browser.performActions([{
    type: "key",
    id: "editor-tab-keyboard",
    actions: [
      ...modifiers.map((value) => ({ type: "keyDown", value })),
      { type: "keyDown", value: key },
      { type: "keyUp", value: key },
      ...modifiers.slice().reverse().map((value) => ({ type: "keyUp", value })),
    ],
  }]);
  await browser.releaseActions();
}

async function runPaletteCommand(browser, title) {
  await chord(browser, [KEY.ctrl, KEY.shift], "p");
  await waitFor(() => browser.execute(() => document.activeElement?.tagName === "INPUT"),
    { timeout: 10_000, label: "command palette input" });
  await typeText(browser, title);
  await browser.keys("Enter");
}

async function typeText(browser, text) {
  await browser.keys(text.split(""));
}

function editorPanelId(observer, slotId = "left") {
  const slot = observer.state.slots[slotId];
  return slot?.tabPanelIds.find((id) => observer.state.panels[id]?.tabKind === "editor");
}

async function expectEditorFocus(browser, label) {
  await waitFor(() => browser.execute(() =>
    Boolean(document.activeElement?.closest(".panel-editor .cm-content"))),
  { timeout: 10_000, label });
}

async function expectListingFocus(browser, panelId, cursorName, label) {
  await waitFor(() => browser.execute(({ panelId, cursorName }) => {
    const panel = document.querySelector(`[data-testid="panel-${panelId}"]`);
    const cursor = panel?.querySelector(".panel__row--cursor, .panel__card--cursor");
    return document.activeElement === panel
      && (cursorName === null || cursor?.getAttribute("data-entry-name") === cursorName);
  }, { panelId, cursorName }), { timeout: 10_000, label });
}

async function editorText(browser) {
  return browser.execute(() => {
    const lines = Array.from(document.querySelectorAll(".panel-editor .cm-line"));
    return lines.map((line) => line.textContent).join("\n");
  });
}

/** Record intents the GUI sends, to prove what a flow does and doesn't use. */
async function recordIntents(browser) {
  await browser.execute(() => {
    window.__editorIntents = [];
    window.__editorOriginalSend = WebSocket.prototype.send;
    WebSocket.prototype.send = function(data) {
      if (typeof data === "string") {
        const message = JSON.parse(data);
        if (message.type === "Intent") window.__editorIntents.push(message.name);
      }
      return window.__editorOriginalSend.call(this, data);
    };
  });
}

async function stopRecordingIntents(browser) {
  return browser.execute(() => {
    const intents = window.__editorIntents ?? [];
    if (window.__editorOriginalSend) WebSocket.prototype.send = window.__editorOriginalSend;
    delete window.__editorOriginalSend;
    delete window.__editorIntents;
    return intents;
  });
}

export async function runEditorTabFlow(browser, observer) {
  const directory = backendPath("a", "src", "editor-tab");
  fs.rmSync(hostPath(directory), { recursive: true, force: true });
  fs.mkdirSync(hostPath(directory), { recursive: true });
  const fileName = "note.md";
  const hostFile = hostPath(path.posix.join(directory, fileName));
  fs.writeFileSync(hostFile, "# Title\r\nfirst line\r\n");

  await goToPanelPath(browser, observer, "left", directory);
  await observer.waitForEntry("left", fileName, "file");
  await clickEntry(browser, "left", fileName);

  // "Edit in Editor Tab" (context menu) opens an editor tab right after the origin listing tab.
  await openPanelContextMenu(browser, "left");
  await clickContextMenuItem(browser, "Edit in Editor Tab");
  await waitFor(() => {
    const slot = observer.state.slots.left;
    const editorId = editorPanelId(observer);
    return editorId && slot.activePanelId === editorId
      && slot.tabPanelIds.indexOf(editorId) === slot.tabPanelIds.indexOf("left") + 1
      && observer.state.focus === editorId;
  }, { timeout: 10_000, label: "editor tab opened after the origin" });
  const editorId = editorPanelId(observer);
  await expectEditorFocus(browser, "editor takes focus");
  await waitFor(async () => (await editorText(browser)).startsWith("# Title"), {
    timeout: 10_000, label: "editor shows the file",
  });

  // Typing autosaves; CRLF line endings are kept. Shift+S types text instead
  // of opening the listing's sort menu.
  await chord(browser, [KEY.ctrl], KEY.end);
  await typeText(browser, "autosaved");
  await chord(browser, [KEY.shift], "s");
  await waitFor(() => fs.readFileSync(hostFile, "utf8") === "# Title\r\nfirst line\r\nautosavedS", {
    timeout: 10_000, label: "autosave writes the typed text with CRLF kept",
  });
  if (await browser.execute(() => Boolean(document.querySelector('[data-testid="panel-sort-selector"]')))) {
    throw new Error("Shift+S in the editor opened the listing sort menu");
  }

  // Escape returns to the origin listing with the cursor on the file; the
  // editor tab stays open.
  await chord(browser, [], KEY.escape);
  await waitFor(() => observer.state.slots.left?.activePanelId === "left"
    && observer.state.focus === "left"
    && observer.state.slots.left.tabPanelIds.includes(editorId),
  { timeout: 10_000, label: "Escape returns to the origin tab" });
  await expectListingFocus(browser, "left", fileName, "listing focus on the edited file after Escape");

  // Back to the editor: undo history survived the tab switch.
  observer.sendIntent("panel.tab.switch", undefined, { slotId: "left", panelId: editorId });
  await expectEditorFocus(browser, "editor focus after switching back");
  // WebDriver maps "z" through the host keyboard layout (Ctrl+Y on QWERTZ),
  // so send Ctrl+Z as the event a real Z key produces.
  await browser.execute(() => {
    document.querySelector(".panel-editor .cm-content")?.dispatchEvent(new KeyboardEvent("keydown", {
      key: "z", code: "KeyZ", keyCode: 90, ctrlKey: true, bubbles: true, cancelable: true,
    }));
  });
  await waitFor(() => !fs.readFileSync(hostFile, "utf8").includes("autosavedS"), {
    timeout: 10_000, label: "undo after a tab switch, then autosave",
  });

  // Shift+J types text (it no longer cycles tabs); Ctrl+Tab / Ctrl+Shift+Tab
  // leave the editor and cycle tabs, saving first; Ctrl+Shift+P opens the
  // palette and Escape hands focus back to CodeMirror.
  await chord(browser, [KEY.shift], "j");
  await waitFor(async () => (await editorText(browser)).includes("J"), {
    timeout: 10_000, label: "Shift+J types in the editor",
  });
  if (observer.state.slots.left?.activePanelId !== editorId) throw new Error("Shift+J switched tabs from the editor");
  const tabIds = observer.state.slots.left.tabPanelIds;
  const editorIndex = tabIds.indexOf(editorId);
  const nextTabId = tabIds[(editorIndex + 1) % tabIds.length];
  await chord(browser, [KEY.ctrl], KEY.tab);
  await waitFor(() => observer.state.slots.left?.activePanelId === nextTabId
    && observer.state.focus === nextTabId,
  { timeout: 10_000, label: "Ctrl+Tab from the editor activates the next tab" });
  // The backend switches first; DOM focus follows once the listing mounts.
  if (observer.state.panels[nextTabId]?.tabKind === "editor") {
    await expectEditorFocus(browser, "editor focus after Ctrl+Tab");
  } else {
    await expectListingFocus(browser, nextTabId, null, "listing focus after Ctrl+Tab from the editor");
  }
  await waitFor(() => fs.readFileSync(hostFile, "utf8").includes("J"), {
    timeout: 10_000, label: "leaving the editor with Ctrl+Tab saves",
  });
  await chord(browser, [KEY.ctrl, KEY.shift], KEY.tab);
  await waitFor(() => observer.state.slots.left?.activePanelId === editorId, {
    timeout: 10_000, label: "Ctrl+Shift+Tab returns to the editor tab",
  });
  await expectEditorFocus(browser, "editor focus after Ctrl+Shift+Tab");
  const beforePalette = await editorText(browser);
  await chord(browser, [KEY.ctrl, KEY.shift], "p");
  await waitFor(() => browser.execute(() =>
    document.activeElement?.tagName === "INPUT" && !document.activeElement.closest(".panel-editor")),
  { timeout: 10_000, label: "Ctrl+Shift+P opens the palette from the editor" });
  await chord(browser, [], KEY.escape);
  await expectEditorFocus(browser, "focus returns to the editor after the palette");
  if (await editorText(browser) !== beforePalette) throw new Error("Ctrl+Shift+P typed into the editor");

  // A fully saved buffer follows external edits silently.
  await sleep(1_500);
  fs.writeFileSync(hostFile, "external one\r\n");
  await waitFor(async () => (await editorText(browser)).startsWith("external one"), {
    timeout: 10_000, label: "clean editor reloads an external change",
  });

  // Unsaved changes plus an external edit: autosave pauses, Ctrl+S resolves.
  await chord(browser, [KEY.ctrl], KEY.end);
  await typeText(browser, "mine");
  fs.writeFileSync(hostFile, "external two\r\n");
  await waitFor(() => browser.execute(() =>
    Boolean(document.querySelector('[data-testid="editor-conflict-bar"]'))),
  { timeout: 10_000, label: "conflict bar after an external change with unsaved text" });
  if (fs.readFileSync(hostFile, "utf8") !== "external two\r\n") {
    throw new Error("Autosave overwrote an external change");
  }
  await chord(browser, [KEY.ctrl], "s");
  await waitFor(() => browser.execute(() =>
    document.activeElement?.getAttribute("data-testid") === "editor-conflict-overwrite"),
  { timeout: 10_000, label: "conflict dialog focuses Overwrite" });
  // The covered editor ignores keys while the dialog is open.
  const beforeCovered = await editorText(browser);
  await typeText(browser, "x");
  if (await editorText(browser) !== beforeCovered) throw new Error("Covered editor received keys");
  await typeText(browser, "r");
  await waitFor(async () => (await editorText(browser)).startsWith("external two")
    && !(await browser.execute(() => Boolean(document.querySelector('[data-testid="editor-conflict-bar"]')))),
  { timeout: 10_000, label: "Reload takes the disk version and resumes autosave" });
  await expectEditorFocus(browser, "focus returns to the editor after the conflict dialog");

  // A long note, so the preview has something to follow (clean buffer: reloads).
  const sections = Array.from({ length: 40 }, (_, i) => `## Section ${i + 1}\r\n\r\nBody of section ${i + 1}.\r\n`);
  fs.writeFileSync(hostFile, `external two\r\n\r\n${sections.join("\r\n")}`);
  await waitFor(async () => (await editorText(browser)).includes("## Section 2"), {
    timeout: 10_000, label: "long note reloaded into the editor",
  });

  // Quick preview follows the editor and refreshes after autosave.
  await chord(browser, [KEY.ctrl], KEY.f3);
  await waitFor(() => browser.execute(() =>
    Boolean(document.querySelector('[data-testid="preview-right"]'))),
  { timeout: 10_000, label: "quick preview opens beside the editor" });
  await expectEditorFocus(browser, "focus stays in the editor with preview open");
  // The preview follows the cursor line: end of the note, then back to the top.
  const sectionVisible = (title) => browser.execute((title) => {
    const scroller = document.querySelector('[data-testid="preview-right"] .viewer__markdown');
    const heading = Array.from(scroller?.querySelectorAll("h2") ?? []).find((h) => h.textContent === title);
    if (!scroller || !heading) return false;
    const box = scroller.getBoundingClientRect();
    const rect = heading.getBoundingClientRect();
    return rect.top >= box.top && rect.bottom <= box.bottom;
  }, title);
  await chord(browser, [KEY.ctrl], KEY.end);
  await waitFor(() => sectionVisible("Section 40"), { timeout: 10_000, label: "preview follows the cursor to the end" });
  await chord(browser, [KEY.ctrl], "\uE011");
  await waitFor(() => sectionVisible("Section 1"), { timeout: 10_000, label: "preview follows the cursor back to the top" });
  await waitFor(() => browser.execute(() =>
    document.querySelector('[data-testid="preview-right"]')?.textContent?.includes("external two")),
  { timeout: 10_000, label: "preview shows the note" });
  const previewSession = () => browser.execute(() =>
    document.querySelector('[data-testid="preview-right"]')?.getAttribute("data-preview-request-id"));
  const sessionBefore = await previewSession();
  await chord(browser, [KEY.ctrl], KEY.end);
  await typeText(browser, "\npreviewed words");
  await waitFor(() => browser.execute(() =>
    document.querySelector('[data-testid="preview-right"]')?.textContent?.includes("previewed words")),
  { timeout: 10_000, label: "preview shows the autosaved text" });
  await typeText(browser, "!");
  await waitFor(() => browser.execute(() =>
    document.querySelector('[data-testid="preview-right"]')?.textContent?.includes("previewed words!")),
  { timeout: 10_000, label: "preview follows a one-letter change" });
  // Autosaves refresh the preview in place: same content session, no remount.
  if (await previewSession() !== sessionBefore) {
    throw new Error("Quick preview remounted its content on an autosave");
  }
  await chord(browser, [KEY.ctrl], KEY.f3);
  await waitFor(() => browser.execute(() => !document.querySelector('[data-testid="preview-right"]')),
    { timeout: 10_000, label: "quick preview closes" });

  // Full screen: Ctrl+Shift+M fills the window, Escape restores first.
  const editorBox = () => browser.execute(() => {
    const el = document.querySelector(".panel-editor");
    const r = el?.getBoundingClientRect();
    return r ? { maximized: el.hasAttribute("data-maximized"), w: r.width, h: r.height, ww: window.innerWidth, wh: window.innerHeight } : null;
  });
  await chord(browser, [KEY.ctrl, KEY.shift], "m");
  await waitFor(async () => {
    const box = await editorBox();
    return box?.maximized && Math.abs(box.w - box.ww) < 2 && Math.abs(box.h - box.wh) < 2;
  }, { timeout: 10_000, label: "editor fills the window" });
  await expectEditorFocus(browser, "editor focus in full screen");
  await chord(browser, [], KEY.escape);
  await waitFor(async () => !(await editorBox())?.maximized, { timeout: 10_000, label: "Escape restores full screen" });
  await expectEditorFocus(browser, "editor focus after restoring");
  if (observer.state.slots.left?.activePanelId !== editorId) {
    throw new Error("Escape in full screen must restore, not leave the editor tab");
  }
  await (await browser.$('[data-testid="editor-maximize"]')).click();
  await waitFor(async () => (await editorBox())?.maximized, { timeout: 10_000, label: "button enters full screen" });
  await chord(browser, [KEY.ctrl, KEY.shift], "m");
  await waitFor(async () => !(await editorBox())?.maximized, { timeout: 10_000, label: "Ctrl+Shift+M restores" });

  // Ctrl+W saves and closes the tab; focus returns to a listing.
  await expectEditorFocus(browser, "editor focus before Ctrl+W");
  await typeText(browser, " closing");
  await chord(browser, [KEY.ctrl], "w");
  await waitFor(() => !observer.state.slots.left?.tabPanelIds.includes(editorId), {
    timeout: 10_000, label: "Ctrl+W closes the editor tab",
  });
  if (!fs.readFileSync(hostFile, "utf8").includes("previewed words! closing")) {
    throw new Error("Closing did not save the last change");
  }
  await expectListingFocus(browser, "left", null, "listing focus after closing the editor");

  // The command palette opens the cursor file too.
  await clickEntry(browser, "left", fileName);
  await runPaletteCommand(browser, "edit_builtin");
  await waitFor(() => {
    const id = editorPanelId(observer);
    return id && observer.state.focus === id;
  }, { timeout: 10_000, label: "palette Edit in Editor Tab opens the file" });
  await expectEditorFocus(browser, "editor focus after opening from the palette");
  await chord(browser, [KEY.ctrl], "w");
  await waitFor(() => !editorPanelId(observer), { timeout: 10_000, label: "palette-opened editor closes" });

  await runHighlightingFlow(browser, observer, directory);
  await runNotesFlow(browser, observer, directory);
}

/** Computed style of the first token whose text is exactly `text`, and of the
 *  plain editor text, so a test can tell highlighted from plain. */
async function tokenStyle(browser, text) {
  return browser.execute((text) => {
    const content = document.querySelector(".panel-editor .cm-content");
    if (!content) return null;
    const span = Array.from(content.querySelectorAll(".cm-line span")).find((el) => el.textContent === text);
    if (!span) return null;
    const style = getComputedStyle(span);
    return { color: style.color, fontWeight: style.fontWeight, plain: getComputedStyle(content).color };
  }, text);
}

async function expectHighlighted(browser, text, label) {
  await waitFor(async () => {
    const style = await tokenStyle(browser, text);
    return style && style.color !== style.plain;
  }, { timeout: 10_000, label });
}

/** CodeMirror keys sent as the events a US keyboard produces: WebDriver maps
 *  punctuation through the host layout. */
async function sendEditorKey(browser, init) {
  await browser.execute((init) => {
    document.querySelector(".panel-editor .cm-content")?.dispatchEvent(new KeyboardEvent("keydown", {
      bubbles: true, cancelable: true, ...init,
    }));
  }, init);
}

async function openInEditorTab(browser, observer, name) {
  await clickEntry(browser, "left", name);
  await openPanelContextMenu(browser, "left");
  await clickContextMenuItem(browser, "Edit in Editor Tab");
  await waitFor(() => {
    const id = editorPanelId(observer);
    return id && observer.state.focus === id;
  }, { timeout: 10_000, label: `${name} opens in an editor tab` });
  await expectEditorFocus(browser, `editor focus on ${name}`);
}

async function closeEditorTab(browser, observer, label) {
  await chord(browser, [KEY.ctrl], "w");
  await waitFor(() => !editorPanelId(observer), { timeout: 10_000, label });
}

async function runHighlightingFlow(browser, observer, directory) {
  const files = {
    "sample.rs": "// sample\nfn main() {\n    let s = \"hi\";\n    println!(\"{}\", s);\n}\n",
    "script.py": "# not a heading\nx = 1\n",
    tool: "#!/bin/sh\necho \"hi\"\n",
  };
  for (const [name, text] of Object.entries(files)) {
    fs.writeFileSync(hostPath(path.posix.join(directory, name)), text);
  }
  await observer.waitForEntry("left", "tool", "file");

  // Rust by file name: keywords, strings and comments are coloured.
  await openInEditorTab(browser, observer, "sample.rs");
  await expectHighlighted(browser, "fn", "Rust keyword highlighted");
  await expectHighlighted(browser, "\"hi\"", "Rust string highlighted");
  await expectHighlighted(browser, "// sample", "Rust comment highlighted");

  // Fold and unfold the block at the cursor (line 2); the Ctrl+Alt Fold All
  // binding is gone, because AltGr types `[` as Ctrl+Alt on Windows.
  await chord(browser, [KEY.ctrl], KEY.home);
  await browser.keys("ArrowDown");
  const foldKey = { key: "{", code: "BracketLeft", keyCode: 219, ctrlKey: true, shiftKey: true };
  const unfoldKey = { key: "}", code: "BracketRight", keyCode: 221, ctrlKey: true, shiftKey: true };
  await sendEditorKey(browser, foldKey);
  await waitFor(() => browser.execute(() => Boolean(document.querySelector(".panel-editor .cm-foldPlaceholder"))),
    { timeout: 10_000, label: "Ctrl+Shift+[ folds the block" });
  await sendEditorKey(browser, unfoldKey);
  await waitFor(() => browser.execute(() => !document.querySelector(".panel-editor .cm-foldPlaceholder")),
    { timeout: 10_000, label: "Ctrl+Shift+] unfolds the block" });
  await sendEditorKey(browser, { key: "[", code: "BracketLeft", keyCode: 219, ctrlKey: true, altKey: true });
  await sleep(300);
  if (await browser.execute(() => Boolean(document.querySelector(".panel-editor .cm-foldPlaceholder")))) {
    throw new Error("Ctrl+Alt+[ folded the file (Fold All must stay unbound)");
  }

  // Brackets close as you type; Backspace removes an empty pair.
  await chord(browser, [KEY.ctrl], KEY.end);
  await typeText(browser, "f(");
  await waitFor(async () => (await editorText(browser)).endsWith("f()"), {
    timeout: 10_000, label: "( inserts a closing )",
  });
  await browser.keys("Backspace");
  await waitFor(async () => (await editorText(browser)).endsWith("f"), {
    timeout: 10_000, label: "Backspace removes the empty pair",
  });
  await closeEditorTab(browser, observer, "Rust editor closes");

  // Python is no longer parsed as Markdown: `#` is a comment, not a bold heading.
  await openInEditorTab(browser, observer, "script.py");
  await expectHighlighted(browser, "# not a heading", "Python comment highlighted");
  const comment = await tokenStyle(browser, "# not a heading");
  if (Number(comment.fontWeight) >= 600) throw new Error("Python comment rendered as a Markdown heading");
  await closeEditorTab(browser, observer, "Python editor closes");

  // No extension: the #! line picks Shell.
  await openInEditorTab(browser, observer, "tool");
  await expectHighlighted(browser, "\"hi\"", "shell string highlighted via #!");
  await closeEditorTab(browser, observer, "shell editor closes");
  await expectListingFocus(browser, "left", null, "listing focus after the highlighting flow");
}

async function runNotesFlow(browser, observer, returnDirectory) {
  const scratchBackend = "/state/a/config/fbro/scratch";
  const scratchHost = path.join(artifactsDir, "fbro-a/state/config/fbro/scratch");
  const notesBefore = fs.existsSync(scratchHost) ? fs.readdirSync(scratchHost) : [];

  // The Scratch source-bar entry sits right after the Journal and opens the notes folder.
  const afterJournal = await browser.execute(() => {
    const journal = document.querySelector('[data-testid="panel-left"] [data-source-id="journal"]');
    return journal?.nextElementSibling?.getAttribute("data-testid");
  });
  if (afterJournal !== "pinned-location-scratch") {
    throw new Error(`Scratch should follow the Journal in the source bar, found ${afterJournal}`);
  }

  await (await browser.$('[data-testid="panel-left"] [data-testid="pinned-location-scratch"]')).click();
  await observer.waitForPanelPath("left", scratchBackend);

  // Ctrl+Shift+F4 creates a note in an editor tab; it never goes through New File.
  await recordIntents(browser);
  let intents;
  try {
    await chord(browser, [KEY.ctrl, KEY.shift], KEY.f4);
    await waitFor(() => {
      const id = editorPanelId(observer);
      return id && observer.state.panels[id]?.editor?.isNote && observer.state.focus === id;
    }, { timeout: 10_000, label: "New Note opens an editor tab" });
    await expectEditorFocus(browser, "new note editor focus");
  } finally {
    intents = await stopRecordingIntents(browser);
  }
  if (!intents.includes("CreateNote") || intents.includes("TouchFile")) {
    throw new Error(`New Note must use CreateNote, not TouchFile: ${JSON.stringify(intents)}`);
  }
  const created = fs.readdirSync(scratchHost).filter((name) => !notesBefore.includes(name));
  if (created.length !== 1 || !created[0].endsWith(".md")) {
    throw new Error(`Expected one new note, found ${JSON.stringify(created)}`);
  }

  // Escape returns to the listing, cursor unchanged; the note tab stays.
  const noteId = editorPanelId(observer);
  await chord(browser, [], KEY.escape);
  await waitFor(() => observer.state.slots.left?.activePanelId === "left"
    && observer.state.slots.left.tabPanelIds.includes(noteId),
  { timeout: 10_000, label: "Escape from a new note returns to the listing" });

  // Closing a new note nobody typed in removes the empty file.
  observer.sendIntent("panel.tab.switch", undefined, { slotId: "left", panelId: noteId });
  await expectEditorFocus(browser, "note editor focus before closing");
  await chord(browser, [KEY.ctrl], "w");
  await waitFor(() => !fs.existsSync(path.join(scratchHost, created[0])), {
    timeout: 10_000, label: "untouched new note discarded on close",
  });

  // An existing note in the Scratch folder opens from the context menu.
  fs.mkdirSync(scratchHost, { recursive: true });
  fs.writeFileSync(path.join(scratchHost, "existing-note.md"), "# Kept\n");
  await observer.waitForEntry("left", "existing-note.md", "file");
  await clickEntry(browser, "left", "existing-note.md");
  await openPanelContextMenu(browser, "left");
  await clickContextMenuItem(browser, "Edit in Editor Tab");
  await waitFor(() => {
    const id = editorPanelId(observer);
    return id && observer.state.panels[id]?.editor?.fileName === "existing-note.md" && observer.state.focus === id;
  }, { timeout: 10_000, label: "existing Scratch note opens from the context menu" });
  await expectEditorFocus(browser, "editor focus on an existing note");
  await chord(browser, [KEY.ctrl], "w");
  await waitFor(() => !editorPanelId(observer), { timeout: 10_000, label: "existing note tab closes" });
  if (!fs.existsSync(path.join(scratchHost, "existing-note.md"))) {
    throw new Error("Closing an existing note must not delete it");
  }

  // In the notes folder, Edit (F4) opens the note in an editor tab instead of
  // the external editor.
  await clickEntry(browser, "left", "existing-note.md");
  await chord(browser, [], KEY.f4);
  await waitFor(() => {
    const id = editorPanelId(observer);
    return id && observer.state.panels[id]?.editor?.fileName === "existing-note.md" && observer.state.focus === id;
  }, { timeout: 10_000, label: "F4 on a note opens the built-in editor" });
  await expectEditorFocus(browser, "editor focus after F4 on a note");
  await chord(browser, [KEY.ctrl], "w");
  await waitFor(() => !editorPanelId(observer), { timeout: 10_000, label: "F4-opened note closes" });

  await runNotesFolderSettingFlow(browser, observer);

  observer.sendIntent("NavigatePanelTo", undefined, { panelId: "left", path: returnDirectory });
  await observer.waitForPanelPath("left", returnDirectory);
  await observer.waitForPanelReady("left");
}

async function runNotesFolderSettingFlow(browser, observer) {
  const customBackend = backendPath("a", "src", "custom-notes");
  const customHost = hostPath(customBackend);
  fs.rmSync(customHost, { recursive: true, force: true });

  // Settings → General → Notes folder: relative paths are rejected inline.
  await chord(browser, [KEY.ctrl], ",");
  const input = await browser.$('[data-testid="settings-notes-folder"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await input.scrollIntoView();
  await input.click();
  await typeText(browser, "relative");
  await browser.keys("Enter");
  await waitFor(() => browser.execute(() =>
    document.querySelector('[data-testid="settings-notes-folder-hint"]')?.textContent?.includes("absolute path")),
  { timeout: 10_000, label: "relative notes folder rejected inline" });
  if (observer.state.settings.general.notesFolder) throw new Error("A relative notes folder was accepted");

  // An absolute folder is accepted; it is created on first use.
  await browser.execute(() => {
    const field = document.querySelector('[data-testid="settings-notes-folder"]');
    field.select();
  });
  await typeText(browser, customBackend);
  await browser.keys("Enter");
  await waitFor(() => observer.state.settings.general.notesFolder === customBackend,
    { timeout: 10_000, label: "notes folder setting saved" });
  await chord(browser, [], KEY.escape);
  await waitFor(() => browser.execute(() => !document.querySelector('[data-testid="settings-notes-folder"]')),
    { timeout: 10_000, label: "settings closed" });

  // New notes and the Scratch entry use the configured folder.
  await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
  await chord(browser, [KEY.ctrl, KEY.shift], KEY.f4);
  await waitFor(() => {
    const id = editorPanelId(observer);
    return id && observer.state.panels[id]?.editor?.filePath?.startsWith(`${customBackend}/`);
  }, { timeout: 10_000, label: "new note created in the configured folder" });
  await expectEditorFocus(browser, "editor focus on a note in the configured folder");
  await typeText(browser, "kept");
  await waitFor(() => fs.existsSync(customHost)
    && fs.readdirSync(customHost).some((name) => fs.readFileSync(path.join(customHost, name), "utf8") === "kept"),
  { timeout: 10_000, label: "note saved in the configured folder" });
  await chord(browser, [KEY.ctrl], "w");
  await waitFor(() => !editorPanelId(observer), { timeout: 10_000, label: "configured-folder note closes" });
  await (await browser.$('[data-testid="panel-left"] [data-testid="pinned-location-scratch"]')).click();
  await observer.waitForPanelPath("left", customBackend);

  // Back to the default for later flows.
  observer.sendIntent("SetGeneralOption", undefined, { key: "notesFolder", value: "" });
  await waitFor(() => observer.state.settings.general.notesFolder === "",
    { timeout: 10_000, label: "notes folder reset to the default" });
}
