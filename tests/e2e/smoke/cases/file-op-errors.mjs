import fs from "node:fs";
import path from "node:path";
import { backendPath, hostPath } from "../config.mjs";
import { clickEntry, closeTerminalJobDialog, confirmOperation, goToPanelPath } from "../browser.mjs";
import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

export async function runFileOpErrorsFlow(browser, observer) {
  const aSrc = backendPath("a", "src");
  const aDst = backendPath("a", "dst");
  const fixture = path.posix.join(aSrc, "file-op-errors");
  const tree = path.posix.join(fixture, "tree");
  const brokenSource = path.posix.join(tree, "blocked.bin");
  const brokenHost = hostPath(brokenSource);
  const destinationTree = hostPath(path.posix.join(aDst, "tree"));

  fs.mkdirSync(hostPath(tree), { recursive: true });
  fs.writeFileSync(brokenHost, "blocked payload\n");
  // The backend runs as uid 1000 in the e2e container, so dropping all read
  // bits forces a real read failure without relying on platform ACLs.
  fs.chmodSync(brokenHost, 0o000);
  fs.writeFileSync(path.join(hostPath(tree), "good.txt"), "copied payload\n");
  fs.mkdirSync(hostPath(aDst), { recursive: true });
  if (fs.existsSync(destinationTree)) fs.rmSync(destinationTree, { recursive: true });

  await goToPanelPath(browser, observer, "left", fixture);
  await goToPanelPath(browser, observer, "right", aDst);
  await observer.waitForEntry("left", "tree", "directory");
  await clickEntry(browser, "left", "tree");
  const cursor = observer.state.panels.left.cursor;

  // Start the job from the GUI connection so job messages reach the app.
  await browser.keys("F5");
  await expectModalFocus(browser, '[data-command-id="file.copy.confirm"]', "copy start dialog focus");
  const errorPolicy = await browser.execute(() =>
    document.querySelector('[data-testid="file-error-policy"]')?.value);
  if (errorPolicy !== "ask") {
    throw new Error(`Expected "Action on error" to default to Ask, got ${errorPolicy}`);
  }
  await confirmOperation(browser, "file.copy.confirm");

  const chip = await browser.$('[data-testid="job-chip"][data-job-kind="file-op"]');
  await chip.waitForDisplayed({ timeout: 20_000 });

  const retry = '[data-command-id="job.decision.retry"]';
  await browser.$(retry).waitForDisplayed({ timeout: 20_000 });
  await expectModalFocus(browser, retry, "error decision focuses Retry");
  await browser.keys("j");
  if (observer.state.panels.left.cursor !== cursor) {
    throw new Error("Panel keys reached the listing under the error dialog");
  }

  const notifications = new Set(observer.state.notifications.entries.map((entry) => entry.id));
  await browser.keys("s");
  await waitFor(() => browser.execute(() => {
    const items = document.querySelector('[data-testid="skipped-items"]');
    return !!items && items.textContent.includes("blocked.bin");
  }), { label: "terminal skipped list contains the failed file" });
  if (fs.existsSync(path.join(destinationTree, "blocked.bin"))) {
    throw new Error("Skipped copy left a destination file");
  }
  if (fs.readFileSync(path.join(destinationTree, "good.txt"), "utf8") !== "copied payload\n") {
    throw new Error("Skipping one file stopped the rest of the folder copy");
  }

  fs.chmodSync(brokenHost, 0o600);
  if (fs.readFileSync(brokenHost, "utf8") !== "blocked payload\n") {
    throw new Error("Skipped copy removed or damaged its source");
  }
  fs.chmodSync(brokenHost, 0o644);
  await waitFor(() => observer.state.notifications.entries
    .some((entry) => !notifications.has(entry.id) && entry.message === "Skipped copying blocked.bin"),
  { label: "explicit Skip publishes a per-item toast" });

  await closeTerminalJobDialog(browser);
  await waitFor(() => browser.execute(() =>
    document.activeElement?.getAttribute("data-testid") === "panel-left"),
  { label: "error dialog restores the originating panel focus" });
}
