import { waitFor } from "../wait.mjs";
import { expectModalFocus } from "./modal-focus.mjs";

export async function verifyJobFiles(browser, observer, jobId, directory) {
  const job = `[data-testid="job-row"][data-job-id="${jobId}"]`;
  const file = (index) => `${job} [data-testid="job-file-row"][data-file-index="${index}"]`;
  const originalScheme = observer.state.settings.general.colorScheme;
  for (const scheme of ["fbro-dark", "daylight"]) {
    observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: scheme });
    await waitFor(() => browser.execute((scheme) => document.querySelector(".app")?.dataset.colorScheme === scheme, scheme),
      { label: `job file theme ${scheme}` });
    await browser.execute(() => document.querySelector('[data-testid="panel-left"]')?.focus());
    await browser.keys("j");
    await browser.$('[data-modal-id="jobs"]').waitForDisplayed();
    await browser.keys("Tab"); // Close → tablist → newest job.
    await browser.keys("Tab");
    await expectModalFocus(browser, job, "Tab enters the newest job header");
    await browser.keys(scheme === "fbro-dark" ? "Enter" : "ArrowRight");
    await expectModalFocus(browser, file(0), "expanding enters the first file");
    const cursor = observer.state.panels.left.cursor;
    await browser.keys("ArrowDown");
    await expectModalFocus(browser, file(1), "Down navigates files");
    await browser.keys("End");
    await expectModalFocus(browser, file(27), "End crosses file pages");
    await browser.keys("PageUp");
    await expectModalFocus(browser, file(2), "PageUp moves one file page");
    await browser.keys("Home");
    await expectModalFocus(browser, file(0), "Home returns to first file");
    await browser.keys("Tab");
    await expectModalFocus(browser, `${file(0)} [data-testid="job-file-copy-path"]`, "Tab visits active file Copy");
    await browser.keys(["Shift", "Tab", "Shift"]);
    await expectModalFocus(browser, file(0), "Shift+Tab returns from Copy to its file");
    await browser.keys("Tab");
    await browser.keys("Tab");
    await expectModalFocus(browser, `${file(0)} [data-testid="job-file-jump"]`, "Tab visits active file Jump");
    await browser.keys(["Shift", "Tab", "Shift"]);
    await expectModalFocus(browser, `${file(0)} [data-testid="job-file-copy-path"]`, "Shift+Tab returns from Jump to Copy");
    await browser.keys("Tab");
    await browser.keys("Tab");
    await waitFor(() => browser.execute((job) => {
      const active = document.activeElement;
      return active?.matches('[data-testid="job-row"]') && !active.matches(job);
    }, job), { label: "Tab leaves files for next job" });
    await browser.keys(["Shift", "Tab", "Shift"]);
    await expectModalFocus(browser, job, "Shift+Tab returns to previous job header");
    await browser.keys("ArrowRight");
    await expectModalFocus(browser, file(0), "Right reenters expanded files");
    await browser.keys("End");
    const copy = await browser.$(`${file(27)} [data-testid="job-file-copy-path"]`);
    if (!(await copy.getAttribute("title")).includes(`${directory}/job-file-28.txt`)) {
      throw new Error("Outgoing job did not expose the actual local file path");
    }
    const historyIds = new Set(observer.state.notifications.entries.map((entry) => entry.id));
    await browser.keys("c");
    await waitFor(() => observer.state.notifications.entries
      .some((entry) => !historyIds.has(entry.id) && entry.message === "Copied full path"),
    { label: "keyboard path copy publishes feedback" });
    await expectModalFocus(browser, file(27), "copy retains active file focus");
    if (observer.state.panels.left.cursor !== cursor) throw new Error("Job file keys reached the covered panel");
    await browser.keys("ArrowLeft");
    await expectModalFocus(browser, job, "Left returns to parent job");
    await browser.keys("ArrowLeft");
    await waitFor(async () => !(await browser.$(`${job} .jobs-modal__file-list`).isExisting()),
      { label: "Left collapses the file list" });
    await browser.keys("Enter");
    await browser.keys("End");
    await browser.keys("g");
    await browser.$('[data-modal-id="jobs"]').waitForDisplayed({ reverse: true });
    await observer.waitForPanelPath("left", directory);
    await waitFor(() => observer.state.panels.left.entries[observer.state.panels.left.cursor]?.name === "job-file-28.txt",
      { label: "jump reveals the active file" });
    await waitFor(() => browser.execute(() => document.activeElement?.matches('[data-testid="panel-left"]')),
      { label: "jump restores listing focus" });
  }
  observer.sendIntent("SetGeneralOption", undefined, { key: "colorScheme", value: originalScheme });
}
