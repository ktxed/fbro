import fs from "node:fs";
import { execFileSync } from "node:child_process";
import { backendPath, hostPath, composeFiles } from "../config.mjs";
import { goToPanelPath, waitForMainWindow } from "../browser.mjs";
import { waitFor } from "../wait.mjs";

export async function runDiffFlow(browser, observer) {
  const originalPath = observer.state.panels.left.path;
  const mainUrl = await browser.getUrl();
  const repo = backendPath("a", "diff-scroll");
  fs.mkdirSync(hostPath(repo), { recursive: true });
  execFileSync("git", ["-C", hostPath(repo), "init", "-q"]);
  fs.writeFileSync(hostPath(`${repo}/new.txt`), "new file line\n".repeat(300));
  await goToPanelPath(browser, observer, "left", repo);
  await observer.waitForEntry("left", "new.txt", "file");
  const sessionIds = () => [...execFileSync("docker", ["compose", ...composeFiles.flatMap(file => ["-f", file]),
    "logs", "--no-color", "fbro-a"], { encoding: "utf8" })
    .matchAll(/GetCompareDiff scope=null args=\{"requestId":"([^"]+)"\}/g)].map(match => match[1]);
  const previousIds = new Set(sessionIds());
  observer.sendIntent("GitViewDiff", undefined, { panelId: "left", targetPath: `${repo}/new.txt` });
  let requestId;
  try {
    // Native windows open in the backend container. Reuse its real compare
    // session in the host WebDriver window, as the viewer smoke flow does.
    await waitFor(() => {
      requestId = sessionIds().find(id => !previousIds.has(id));
      return !!requestId;
    }, { timeout: 15_000, label: "new-file diff session" });
    await browser.url(`${mainUrl.split("?")[0]}?diff=1&request=${encodeURIComponent(requestId)}`);
    await browser.$('[data-diff-row="299"]').waitForExist({ timeout: 10_000 });
    for (const mode of ["s", "u"]) {
      await browser.keys(mode);
      await browser.keys("Home");
      const position = () => browser.execute(() => {
        const left = document.querySelector('[data-testid="diff-split-left"]');
        const right = document.querySelector('[data-testid="diff-split-right"]');
        return { top: (left ?? document.querySelector(".diff-viewer__body")).scrollTop,
          right: right?.scrollTop };
      });
      await browser.keys("ArrowDown");
      await waitFor(async () => (await position()).top > 0,
        { timeout: 3000, label: `${mode} Down scrolls a whole new file` });
      const down = await position();
      if (mode === "s" && down.top !== down.right) throw new Error("Diff panes lost scroll synchronization");
      await browser.keys("ArrowUp");
      await waitFor(async () => (await position()).top < down.top,
        { timeout: 3000, label: `${mode} Up scrolls a whole new file` });
    }
  } finally {
    if (requestId) observer.sendIntent("CloseCompareWindow", undefined, { requestId, label: `diff-${requestId}` });
    await browser.url(mainUrl);
    await waitForMainWindow(browser, "diff smoke return");
    await goToPanelPath(browser, observer, "left", originalPath);
  }
}
