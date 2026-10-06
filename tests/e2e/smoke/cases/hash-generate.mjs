import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { goToPanelPath, pressPanelKey } from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

const BRO_ID = "builtin:generate-hashes";
const EMPTY_HASH = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

export async function runHashGenerateFlow(browser, observer) {
  const root = backendPath("a", "src", `hash-generate-${Date.now()}`);
  const write = (name, body) => fs.writeFileSync(hostPath(path.posix.join(root, name)), body);
  fs.mkdirSync(hostPath(root), { recursive: true });
  write("a.y", ""); write("å file .bin", ""); write("existing", ""); write("existing.b3", "preserve me"); write("orphan.b3", EMPTY_HASH);
  fs.mkdirSync(hostPath(path.posix.join(root, "folder")));
  write("folder/nested", "");
  await goToPanelPath(browser, observer, "left", root);
  observer.sendIntent("SelectAll", "left");
  await waitFor(() => observer.state.panels.left.selection.length === 6, { label: "mixed generation selection" });
  await launch(browser, false);
  await completed(browser);
  assert.deepEqual((await statuses(browser)).sort(), ["created", "created", "skipped"]);
  assert.match(await (await browser.$('[data-testid="hash-generate-summary"]')).getText(), /3 ignored/);
  assert.equal(fs.readFileSync(hostPath(path.posix.join(root, "existing.b3")), "utf8"), "preserve me");
  for (const name of ["a.y", "å file .bin"]) assert.equal(fs.readFileSync(hostPath(path.posix.join(root, `${name}.b3`)), "utf8"), `${EMPTY_HASH}  ${name}\n`);
  assert.equal(fs.existsSync(hostPath(path.posix.join(root, "orphan.b3.b3"))), false);
  assert.equal(fs.existsSync(hostPath(path.posix.join(root, "folder/nested.b3"))), false);
  assert.equal(await browser.execute(() => document.querySelector('[data-testid="hash-generate-dialog"] .modal__actions').querySelectorAll("button").length), 1);
  await expectModalFocus(browser, '[data-testid="hash-generate-dialog"] [data-command-id="job.close"]', "generation completion retains close focus");
  await waitFor(() => browser.execute(() => document.querySelector('[data-testid="transient-hint"]')?.textContent.includes("Saved 2 hash files")), { label: "generated hash toast" });
  await browser.keys("Enter"); await panelFocus(browser);
  await waitFor(() => observer.state.panels.left.entries.some((entry) => entry.name === "a.y.b3"), { label: "generated hash appears in panel" });

  await selectNames(observer, ["existing"]);
  await launch(browser, true); await completed(browser);
  assert.deepEqual(await statuses(browser), ["replaced"]);
  assert.equal(fs.readFileSync(hostPath(path.posix.join(root, "existing.b3")), "utf8"), `${EMPTY_HASH}  existing\n`);
  assert.equal(fs.readdirSync(hostPath(root)).some((name) => name.startsWith(".fbro-hash-")), false);
  await browser.keys("Escape"); await panelFocus(browser);

  // All newly created/replaced sidecars must be accepted by the checker.
  await selectNames(observer, ["a.y.b3", "å file .bin.b3", "existing.b3"]);
  await openBro(browser, "builtin:check-hashes");
  await browser.keys("Enter"); await browser.keys("Tab"); await browser.keys("Enter");
  await waitFor(() => browser.execute(() => document.querySelector('[data-testid="hash-check-dialog"] .modal__header')?.textContent.includes("Completed")), { label: "generated hashes round trip" });
  assert.deepEqual((await browser.execute(() => Array.from(document.querySelectorAll('[data-testid="hash-check-row"]'), (row) => row.getAttribute("data-status")))).sort(), ["matched", "matched", "matched"]);
  await browser.keys("Escape"); await panelFocus(browser);

  // Eligibility checks the complete selection even when a ranking sample misses the only input.
  const bulk = path.posix.join(root, "bulk"); fs.mkdirSync(hostPath(bulk));
  for (let index = 0; index < 120; index += 1) fs.writeFileSync(hostPath(path.posix.join(bulk, `hash-${index}.b3`)), EMPTY_HASH);
  fs.writeFileSync(hostPath(path.posix.join(bulk, "z-file")), "");
  await goToPanelPath(browser, observer, "left", bulk); observer.sendIntent("SelectAll", "left");
  await waitFor(() => observer.state.panels.left.selection.length === 121, { label: "full discovery selection" });
  const requestId = `generation-discovery-${Date.now()}`;
  observer.sendIntent("ListBros", "left", { requestId });
  const options = await observer.waitForBroOptions(requestId);
  assert.equal(options.bros[0].id, "builtin:check-hashes");
  assert.ok(options.bros.some((bro) => bro.id === BRO_ID));
  await goToPanelPath(browser, observer, "left", root);
  observer.sendIntent("LaunchBro", undefined, { requestId, broId: BRO_ID });
  await waitFor(() => observer.messages.some((message) => message.type === "Error" && message.code === "BRO_LAUNCH_FAILED" && message.extra?.requestId === requestId), { label: "stale generation launch rejected" });

  // An 8 GiB sparse source gives a stable opportunity to background, reopen and cancel.
  const large = path.posix.join(root, "large"); fs.mkdirSync(hostPath(large));
  const fd = fs.openSync(hostPath(path.posix.join(large, "large.bin")), "w"); fs.ftruncateSync(fd, 8 * 1024 ** 3); fs.closeSync(fd);
  fs.writeFileSync(hostPath(path.posix.join(large, "large.bin.b3")), "previous hash");
  await goToPanelPath(browser, observer, "left", large); await selectNames(observer, ["large.bin"]);
  await launch(browser, true);
  await waitFor(() => browser.execute(() => {
    const bar = document.querySelector('[data-testid="hash-generate-dialog"] [role="progressbar"][aria-label="Current file"]');
    const percent = Number(bar?.getAttribute("aria-valuenow")); return percent > 0 && percent < 100;
  }), { timeout: 30_000, interval: 50, label: "generation byte progress" });
  await browser.keys("Escape"); await panelFocus(browser);
  const chip = await browser.$('[data-testid="job-chip"][data-job-kind="hash-generate-bro"][data-job-state="Running"]');
  await chip.waitForDisplayed({ timeout: 5_000 }); const jobId = await chip.getAttribute("data-job-id");
  await chip.click();
  await expectModalFocus(browser, '[data-testid="hash-generate-dialog"] [data-command-id="job.close"]', "reopened generation focus");
  await browser.keys("Enter"); await panelFocus(browser);
  await (await browser.$(`[data-testid="job-chip"][data-job-id="${jobId}"] .jobs-bar__job-close`)).click();
  await waitFor(() => browser.execute((jobId) => !document.querySelector(`[data-testid="job-chip"][data-job-id="${jobId}"][data-job-state="Running"]`), jobId), { timeout: 15_000, label: "cancel generation through Jobs" });
  assert.equal(fs.readFileSync(hostPath(path.posix.join(large, "large.bin.b3")), "utf8"), "previous hash");
  assert.equal(fs.readdirSync(hostPath(large)).some((name) => name.startsWith(".fbro-hash-")), false);
  await goToPanelPath(browser, observer, "left", backendPath("a", "src"));
}

async function openBro(browser, broId = BRO_ID) {
  await pressPanelKey(browser, "left", "F1");
  await (await browser.$('[data-testid="bro-dialog"]')).waitForDisplayed({ timeout: 10_000 });
  await (await browser.$(`[data-testid="bro-option"][data-bro-id="${broId}"]`)).waitForExist({ timeout: 10_000 });
  // Keep selection/navigation keyboard-driven even when other Bros rank above generation.
  for (let index = 0; index < 30; index += 1) {
    if (await browser.execute((broId) => document.querySelector('[data-testid="bro-option"][data-selected="true"]')?.getAttribute("data-bro-id") === broId, broId)) return;
    await browser.keys("ArrowDown");
  }
  throw new Error(`Cannot focus ${broId}`);
}
async function launch(browser, overwrite) {
  await openBro(browser); await browser.keys("Enter");
  await expectModalFocus(browser, '[data-testid="bro-param-overwriteExisting"]', "overwrite checkbox focus");
  assert.equal(await (await browser.$('[data-testid="bro-param-overwriteExisting"]')).isSelected(), false);
  if (overwrite) await browser.keys("Space");
  await browser.keys("Tab"); await expectModalFocus(browser, '[data-testid="bro-launch-button"]', "generation launch focus");
  await browser.keys("Enter");
}
async function completed(browser) {
  await waitFor(() => browser.execute(() => document.querySelector('[data-testid="hash-generate-dialog"] .modal__header')?.textContent.includes("Completed")), { label: "hash generation completed" });
}
async function statuses(browser) { return browser.execute(() => Array.from(document.querySelectorAll('[data-testid="hash-generate-row"]'), (row) => row.getAttribute("data-status"))); }
async function selectNames(observer, names) {
  observer.sendIntent("ClearSelection", "left"); await waitFor(() => observer.state.panels.left.selection.length === 0, { label: "clear generation selection" });
  for (const name of names) {
    const index = observer.state.panels.left.entries.findIndex((entry) => entry.name === name); assert.ok(index >= 0, `${name} is listed`);
    observer.sendIntent("ToggleSelectionAtIndex", "left", { index });
  }
  await waitFor(() => observer.state.panels.left.selection.length === names.length, { label: "select generation inputs" });
}
async function panelFocus(browser) {
  await waitFor(() => browser.execute(() => document.activeElement === document.querySelector('[data-testid="panel-left"]')
    && !document.querySelector('[data-testid="hash-generate-dialog"]') && !document.querySelector('[data-testid="hash-check-dialog"]') && !document.querySelector('[data-testid="bro-dialog"]')), { label: "hash modal restored panel focus" });
}
