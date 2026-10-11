import { useCallback } from "react";
import { useNotifications } from "../notifications/service";

/** Returns `copy(text, what)`: writes `text` to the system clipboard and
 *  shows a "Copied <what>" toast (or a failure toast if the write fails).
 *  Resolves to whether the write succeeded, for callers with extra UI state. */
export function useCopyToClipboard() {
  const notify = useNotifications();
  return useCallback((text: string, what: string): Promise<boolean> => (
    navigator.clipboard.writeText(text).then(
      () => {
        notify({ message: `Copied ${what}` });
        return true;
      },
      () => {
        notify({ message: `Couldn't copy ${what} to the clipboard`, level: "error" });
        return false;
      },
    )
  ), [notify]);
}
