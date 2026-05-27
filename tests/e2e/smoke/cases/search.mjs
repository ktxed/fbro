import { backendPath } from "../config.mjs";
import { clickCommand, reloadBrowserApp } from "../browser.mjs";

export async function runSearchFlow(browser, observer) {
  const aDst = backendPath("a", "dst");
  observer.navigatePanelToPath("left", aDst);
  await observer.waitForPanelPath("left", aDst);
  await observer.waitForPanelNotShadow("left");
  await reloadBrowserApp(browser, "fbro-a");
  await browser.execute(() => {
    window.dispatchEvent(new KeyboardEvent("keydown", {
      key: "f",
      code: "KeyF",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    }));
  });
  const input = await browser.$('[data-testid="search-file-mask-input"]');
  await input.waitForDisplayed({ timeout: 10_000 });
  await input.setValue("renamed.txt");
  await clickCommand(browser, "search.start");
  await clickCommand(browser, "search.to-results");
  await observer.waitForSearchResultsMounted("left");
  await observer.waitForEntry("left", "renamed.txt", "file");
  await browser.waitUntil(async () => {
    const dateCell = await browser.$('[data-testid="panel-left"] [data-entry-name="renamed.txt"] .panel__mtime');
    if (!await dateCell.isExisting()) return false;
    const text = await dateCell.getText();
    return text.trim().length > 0;
  }, {
    timeout: 5_000,
    timeoutMsg: "Mounted search result Date column stayed empty",
  });
}
