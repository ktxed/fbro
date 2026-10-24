import assert from "node:assert/strict";
import fs from "node:fs";
import { clickSortHeader, clickSource, goToPanelPath, reloadBrowserApp } from "../browser.mjs";
import { backendPath, hostPath } from "../config.mjs";
import { waitFor } from "../wait.mjs";

const entryIds = (panel) => panel.entries.map((entry) => entry.sourcePath).sort();

async function navigate(browser, observer, panelId, sourceId, targetPath) {
  await clickSource(browser, panelId, sourceId);
  await waitFor(() => {
    observer.sendIntent("RequestStateSnapshot");
    return observer.state.panels[panelId]?.sourceId === sourceId
      && observer.state.panels[panelId]?.status === "ready";
  }, { label: `${panelId} source ${sourceId}` });
  if (targetPath !== undefined) {
    await goToPanelPath(browser, observer, panelId, targetPath);
    observer.sendIntent("RequestStateSnapshot");
    await observer.waitForPanelPath(panelId, targetPath);
    await observer.waitForPanelReady(panelId);
  }
}

async function remember(observer, value) {
  observer.sendIntent("SetGeneralOption", undefined, { key: "rememberSortPerLocation", value });
  await waitFor(() => observer.state.settings.general.rememberSortPerLocation === value,
    { label: `Journal remember sorting ${value}` });
}

async function expectOrder(browser, observer, panelId, by, order) {
  await observer.waitForSort(panelId, by, order);
  await waitFor(() => {
    const panel = observer.state.panels[panelId];
    if (panel.status !== "ready" || panel.entries.length < 2) return false;
    const values = panel.entries.map((entry) => by === "date" ? Number(entry.mtime)
      : by === "size" ? entry.size : entry.name);
    return values.every((value, i) => i === 0 || (order === "asc"
      ? values[i - 1] <= value : values[i - 1] >= value));
  }, { label: `${panelId} actual Journal ${by} ${order} order` });
  const expectedNames = observer.state.panels[panelId].entries.map((entry) => entry.name);
  try {
    await waitFor(() => browser.execute((panelId, by, order, expectedNames) => {
      const panel = document.querySelector(`[data-testid="panel-${panelId}"]`);
      const active = panel?.querySelector(`.panel__sort-${by} .panel__sort-tri--active`);
      const direction = order === "asc" ? "up" : "down";
      const names = Array.from(panel?.querySelectorAll("[data-entry-name]") ?? [],
        (row) => row.getAttribute("data-entry-name"));
      const positions = names.map((name) => expectedNames.indexOf(name));
      return !!active?.querySelector(`.tabler-icon-chevron-${direction}`) && names.length > 0
        && positions.every((position, i) => position >= 0 && (i === 0 || positions[i - 1] < position));
    }, panelId, by, order, expectedNames), { label: `${panelId} rendered Journal ${by} ${order} header and rows` });
  } catch (error) {
    const actual = await browser.execute((panelId) => {
      const panel = document.querySelector(`[data-testid="panel-${panelId}"]`);
      return { header: panel?.querySelector(".panel__sort-header")?.outerHTML,
        names: Array.from(panel?.querySelectorAll("[data-entry-name]") ?? [], (row) => row.getAttribute("data-entry-name")) };
    }, panelId);
    throw new Error(`${error.message}; rendered=${JSON.stringify(actual)}; expected=${JSON.stringify(expectedNames)}`);
  }
}

async function setSort(browser, observer, panelId, by, order) {
  // Use the real header control, including a second click when changing a
  // different column directly to descending order.
  if (observer.state.panels[panelId].sort.by !== by
    || observer.state.panels[panelId].sort.order !== order) {
    const firstOrder = observer.state.panels[panelId].sort.by === by
      && observer.state.panels[panelId].sort.order === "asc" ? "desc" : "asc";
    await clickSortHeader(browser, panelId, by);
    await observer.waitForSort(panelId, by, firstOrder);
    if (observer.state.panels[panelId].sort.order !== order) {
      await clickSortHeader(browser, panelId, by);
    }
  }
  await expectOrder(browser, observer, panelId, by, order);
}

// The focused runner has a fresh fixture but no preceding file-operation flow.
export async function runJournalSortFlow(browser, observer) {
  const directory = backendPath("a", "src");
  const original = { ...observer.state.panels.left };
  const names = [`journal-sort-${Date.now()}`, `journal-sort-longer-payload-${Date.now()}`];
  try {
    const sourceId = observer.sourceIdForPath(directory);
    assert.ok(sourceId, "Journal fixture local source exists");
    await navigate(browser, observer, "left", sourceId, directory);
    if (observer.state.settings.general.locationSettingsCount > 0) {
      observer.sendIntent("ResetLocationSortSettings");
      await waitFor(() => observer.state.settings.general.locationSettingsCount === 0,
        { label: "reset isolated focused Journal fixture preferences" });
    }
    const started = Date.now();
    for (const [index, name] of names.entries()) {
      // The wire mtime exposes seconds; ensure the fixture can distinguish
      // ascending and descending dates without depending on machine speed.
      if (index > 0) await waitFor(() => Date.now() - started >= 1_100,
        { label: "distinct Journal fixture date seconds" });
      observer.sendIntent("MakeDir", undefined, { sourcePanel: "left", dirPath: directory, name });
      await observer.waitForEntry("left", name, "directory");
    }
    await verifyJournalSort(browser, observer);
  } finally {
    await navigate(browser, observer, "left", original.sourceId, original.path);
    for (const name of names) fs.rmSync(hostPath(`${directory}/${name}`), { recursive: true, force: true });
  }
}

async function refresh(browser, observer, panelId) {
  const start = observer.messages.length;
  await browser.execute((panelId) => document.querySelector(`[data-testid="panel-${panelId}"]`)?.focus(), panelId);
  await browser.keys("r");
  await waitFor(() => observer.messages.slice(start).some((message) => message.type === "StatePatch"
    && message.domain === "panels" && message.entityId === panelId && message.patch?.status === "ready"),
  { label: `${panelId} Journal keyboard refresh` });
}

// Run after local.mjs's reset assertions: this owns every cache row it creates.
// Real earlier file operations provide varied names, dates and payload sizes.
export async function verifyJournalSort(browser, observer) {
  assert.equal(observer.state.settings.general.locationSettingsCount, 0,
    "Journal smoke starts after the existing location reset");
  const saved = Object.fromEntries(["left", "right"].map((id) => [id, {
    sourceId: observer.state.panels[id].sourceId, path: observer.state.panels[id].path,
  }]));
  const originalRemember = observer.state.settings.general.rememberSortPerLocation;
  let createdTab;
  try {
    await remember(observer, true);
    // Journal's virtual '/' must never alias a real filesystem '/' preference.
    await navigate(browser, observer, "left", saved.left.sourceId, "/");
    observer.sendIntent("SetSort", "left", { by: "size", order: "desc" });
    await observer.waitForSort("left", "size", "desc");
    await navigate(browser, observer, "left", "journal");
    await expectOrder(browser, observer, "left", "name", "asc");
    const initialIds = entryIds(observer.state.panels.left);
    assert.ok(initialIds.length >= 2, "Journal needs real earlier operation events");
    for (const by of ["name", "size", "date"]) {
      for (const order of ["desc", "asc"]) await setSort(browser, observer, "left", by, order);
      const values = observer.state.panels.left.entries.map((entry) => by === "date" ? entry.mtime
        : by === "size" ? entry.size : entry.name);
      assert.ok(new Set(values).size > 1, `Journal fixture has varied ${by} values`);
    }
    await navigate(browser, observer, "left", saved.left.sourceId, "/");
    await observer.waitForSort("left", "size", "desc");
    await navigate(browser, observer, "left", "journal");
    await expectOrder(browser, observer, "left", "date", "asc");
    await refresh(browser, observer, "left");
    await expectOrder(browser, observer, "left", "date", "asc");
    await navigate(browser, observer, "right", "journal");
    await expectOrder(browser, observer, "right", "date", "asc");
    await setSort(browser, observer, "left", "date", "desc");
    await expectOrder(browser, observer, "right", "date", "desc");
    await waitFor(() => observer.state.settings.general.locationSettingsCount === 2,
      { label: "separate Journal and filesystem root preferences" });

    // Disabling neither shares subsequent changes nor overwrites the cache.
    await remember(observer, false);
    await setSort(browser, observer, "left", "size", "asc");
    await expectOrder(browser, observer, "right", "date", "desc");
    await navigate(browser, observer, "left", saved.left.sourceId, "/");
    await observer.waitForSort("left", "size", "asc");
    await navigate(browser, observer, "left", "journal");
    await expectOrder(browser, observer, "left", "size", "asc");
    assert.equal(observer.state.settings.general.locationSettingsCount, 2);
    await remember(observer, true);
    await refresh(browser, observer, "left");
    await expectOrder(browser, observer, "left", "date", "desc");

    const previousTabs = [...observer.state.slots.left.tabPanelIds];
    observer.sendIntent("panel.tab.new", undefined, { slotId: "left", source: "clone" });
    await waitFor(() => observer.state.slots.left.tabPanelIds.length === previousTabs.length + 1,
      { label: "Journal sort new listing tab" });
    createdTab = observer.state.slots.left.tabPanelIds.find((id) => !previousTabs.includes(id));
    await navigate(browser, observer, createdTab, "journal");
    await expectOrder(browser, observer, createdTab, "date", "desc");
    await setSort(browser, observer, createdTab, "date", "asc");
    await observer.waitForSort("left", "date", "asc");
    await expectOrder(browser, observer, "right", "date", "asc");
    await reloadBrowserApp(browser, "Journal sorting");
    await expectOrder(browser, observer, createdTab, "date", "asc");
    await refresh(browser, observer, createdTab);
    assert.deepEqual(entryIds(observer.state.panels[createdTab]), initialIds,
      "ordinary sort changes must not add Journal events");

    // Reset removes both remembered keys and adds real audit events. Their
    // arrival must preserve the current ascending order rather than prepend.
    await remember(observer, false);
    observer.sendIntent("ResetLocationSortSettings");
    await waitFor(() => observer.state.settings.general.locationSettingsCount === 0,
      { label: "reset removes Journal and filesystem preferences" });
    await refresh(browser, observer, createdTab);
    await waitFor(() => observer.state.panels[createdTab].entries.length > initialIds.length,
      { label: "new Journal reset events are visible" });
    await expectOrder(browser, observer, createdTab, "date", "asc");
    await remember(observer, true);
    await navigate(browser, observer, "right", saved.right.sourceId, saved.right.path);
    await navigate(browser, observer, "right", "journal");
    await expectOrder(browser, observer, "right", "name", "asc");
  } finally {
    if (createdTab) {
      observer.sendIntent("panel.tab.close", undefined, { panelId: createdTab });
      await waitFor(() => !observer.state.slots.left.tabPanelIds.includes(createdTab),
        { label: "remove Journal sort test tab" });
    }
    if (observer.state.settings.general.locationSettingsCount > 0) {
      observer.sendIntent("ResetLocationSortSettings");
      await waitFor(() => observer.state.settings.general.locationSettingsCount === 0,
        { label: "clean Journal sort test preferences" });
    }
    for (const [panelId, location] of Object.entries(saved)) {
      await navigate(browser, observer, panelId, location.sourceId, location.path);
    }
    await remember(observer, originalRemember);
  }
}
