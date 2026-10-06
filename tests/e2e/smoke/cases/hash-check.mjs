import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { goToPanelPath, pressPanelKey } from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

const BRO_ID = "builtin:check-hashes";
const EMPTY_HASH = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

export async function runHashCheckFlow(browser, observer) {
  const root = backendPath("a", "src", `hash-check-${Date.now()}`);
  const write = (name, body) => fs.writeFileSync(hostPath(path.posix.join(root, name)), body);
  fs.mkdirSync(hostPath(root), { recursive: true });
  write("a.y", "");
  write("a.y.b3", `${EMPTY_HASH}  a.y\n`);
  write("changed.bin", "changed contents\n");
  write("changed.bin.b3", EMPTY_HASH.toUpperCase());
  write("malformed", "sample");
  write("malformed.b3", "invalid digest");
  write("orphan.b3", EMPTY_HASH);
  write("unrelated.txt", "unrelated");
  await goToPanelPath(browser, observer, "left", root);
  observer.sendIntent("SelectAll", "left");
  await waitFor(() => observer.state.panels.left.selection.length === 8, { label: "hash-check mixed selection" });

  await openBro(browser);
  await browser.keys("Enter");
  await expectModalFocus(browser, '[data-testid="bro-param-saveResult"]', "hash save checkbox focus");
  assert.equal(await (await browser.$('[data-testid="bro-param-saveResult"]')).isSelected(), false);
  await browser.keys("Space");
  assert.equal(await (await browser.$('[data-testid="bro-param-saveResult"]')).isSelected(), true);
  await browser.keys("Tab");
  await expectModalFocus(browser, '[data-testid="bro-launch-button"]', "hash launch focus");
  await browser.keys("Enter");
  await waitFor(() => browser.execute(() => {
    const dialog = document.querySelector('[data-testid="hash-check-dialog"]');
    return dialog?.querySelector(".modal__header")?.textContent.includes("Completed")
      && dialog.querySelector('[data-testid="hash-check-saved"]');
  }), { label: "hash check completed and report saved" });
  const statuses = await browser.execute(() => Array.from(document.querySelectorAll('[data-testid="hash-check-row"]'), (row) => row.getAttribute("data-status")));
  assert.deepEqual(statuses.sort(), ["error", "matched", "mismatched"]);
  assert.match(await (await browser.$('[data-testid="hash-check-summary"]')).getText(), /5 ignored/);
  assert.equal(await browser.execute(() => document.querySelector('[data-testid="hash-check-dialog"] .modal__actions').querySelectorAll("button").length), 1);
  await expectModalFocus(browser, '[data-testid="hash-check-dialog"] [data-command-id="job.close"]', "hash results close focus");
  await waitFor(() => browser.execute(() => document.querySelector('[data-testid="transient-hint"]')?.textContent.includes("Saved hash check report")), { label: "saved report toast" });
  const reports = () => fs.readdirSync(hostPath(root)).filter((name) => name.startsWith("fbro-hash-check-"));
  assert.equal(reports().length, 1);
  const report = fs.readFileSync(hostPath(path.posix.join(root, reports()[0])), "utf8");
  assert.match(report, /Matches: 1/);
  assert.match(report, /Mismatches: 1/);
  assert.match(report, /Errors: 1/);
  await browser.keys("Enter");
  await expectPanelFocus(browser);

  // An orphan sidecar offers the Bro and produces an empty result without saving.
  await selectNames(observer, ["orphan.b3"]);
  await openBro(browser);
  await browser.keys("Enter");
  await browser.keys("Tab");
  await browser.keys("Enter");
  await (await browser.$('[data-testid="hash-check-empty"]')).waitForDisplayed({ timeout: 10_000 });
  await waitFor(() => browser.execute(() => document.querySelector('[data-testid="hash-check-dialog"] .modal__header')?.textContent.includes("Completed")), { label: "orphan check completed" });
  assert.equal(reports().length, 1);
  await browser.keys("Escape");
  await expectPanelFocus(browser);

  // Presence detection must inspect beyond the 100-entry discovery sample.
  const bulk = path.posix.join(root, "bulk");
  fs.mkdirSync(hostPath(bulk));
  for (let index = 0; index < 120; index += 1) fs.writeFileSync(hostPath(path.posix.join(bulk, `item-${index}.txt`)), "");
  fs.writeFileSync(hostPath(path.posix.join(bulk, "z-orphan.b3")), EMPTY_HASH);
  await goToPanelPath(browser, observer, "left", bulk);
  observer.sendIntent("SelectAll", "left");
  await waitFor(() => observer.state.panels.left.selection.length === 121, { label: "large hash selection" });
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await openBro(browser);
    await browser.keys("Escape");
    await expectPanelFocus(browser);
  }

  const staleRequestId = `hash-stale-${Date.now()}`;
  observer.sendIntent("ListBros", "left", { requestId: staleRequestId });
  const options = await observer.waitForBroOptions(staleRequestId);
  assert.equal(options.bros[0].id, BRO_ID);
  assert.equal(options.bros[0].params[0].kind, "bool");
  await goToPanelPath(browser, observer, "left", root);
  observer.sendIntent("LaunchBro", undefined, { requestId: staleRequestId, broId: BRO_ID });
  await waitFor(() => observer.messages.some((message) => message.type === "Error"
    && message.code === "BRO_LAUNCH_FAILED" && message.extra?.requestId === staleRequestId), { label: "stale hash-check launch rejected" });

  // A sparse file exercises byte progress and Close/Escape while still running.
  const large = path.posix.join(root, "large");
  fs.mkdirSync(hostPath(large));
  const fd = fs.openSync(hostPath(path.posix.join(large, "large.bin")), "w");
  fs.ftruncateSync(fd, 8 * 1024 ** 3);
  fs.closeSync(fd);
  fs.writeFileSync(hostPath(path.posix.join(large, "large.bin.b3")), "0".repeat(64));
  await goToPanelPath(browser, observer, "left", large);
  await selectNames(observer, ["large.bin.b3"]);
  await openBro(browser);
  await browser.keys("Enter");
  await browser.keys("Tab");
  await browser.keys("Enter");
  await waitFor(() => browser.execute(() => {
    const bar = document.querySelector('[data-testid="hash-check-dialog"] [role="progressbar"][aria-label="Current file"]');
    const percent = Number(bar?.getAttribute("aria-valuenow"));
    return percent > 0 && percent < 100;
  }), { timeout: 30_000, interval: 50, label: "large-file intermediate hash progress" });
  await browser.keys("Escape");
  await expectPanelFocus(browser);
  const chip = await browser.$('[data-testid="job-chip"][data-job-kind="hash-check-bro"][data-job-state="Running"]');
  await chip.waitForDisplayed({ timeout: 5_000 });
  const jobId = await chip.getAttribute("data-job-id");
  await chip.click();
  await expectModalFocus(browser, '[data-testid="hash-check-dialog"] [data-command-id="job.close"]', "reopened hash check close focus");
  await browser.keys("Enter");
  await expectPanelFocus(browser);
  await (await browser.$(`[data-testid="job-chip"][data-job-id="${jobId}"] .jobs-bar__job-close`)).click();
  await waitFor(() => browser.execute((jobId) => !document.querySelector(`[data-testid="job-chip"][data-job-id="${jobId}"][data-job-state="Running"]`), jobId), { timeout: 15_000, label: "hash check cancelled through jobs" });
  assert.equal(fs.readdirSync(hostPath(large)).filter((name) => name.startsWith("fbro-hash-check-")).length, 0);
  await goToPanelPath(browser, observer, "left", backendPath("a", "src"));
}

async function openBro(browser) {
  await pressPanelKey(browser, "left", "F1");
  await (await browser.$('[data-testid="bro-dialog"]')).waitForDisplayed({ timeout: 10_000 });
  await waitFor(() => browser.execute((broId) => document.querySelector('[data-testid="bro-option"]')?.getAttribute("data-bro-id") === broId, BRO_ID), { label: "Check hashes first in Bro list" });
}

async function selectNames(observer, names) {
  observer.sendIntent("ClearSelection", "left");
  await waitFor(() => observer.state.panels.left.selection.length === 0, { label: "clear hash selection" });
  for (const name of names) {
    const index = observer.state.panels.left.entries.findIndex((entry) => entry.name === name);
    assert.ok(index >= 0, `${name} is listed`);
    observer.sendIntent("ToggleSelectionAtIndex", "left", { index });
  }
  await waitFor(() => observer.state.panels.left.selection.length === names.length, { label: "select hash inputs" });
}

async function expectPanelFocus(browser) {
  await waitFor(() => browser.execute(() => document.activeElement === document.querySelector('[data-testid="panel-left"]')
    && !document.querySelector('[data-testid="hash-check-dialog"]')
    && !document.querySelector('[data-testid="bro-dialog"]')), { label: "hash modal restored panel focus" });
}
