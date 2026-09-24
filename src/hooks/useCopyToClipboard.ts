import { useCallback } from "react";
import { useAppDispatch } from "../store";

/** Returns `copy(text, what)`: writes `text` to the system clipboard and
 *  shows a "Copied <what>" toast (or a failure toast if the write fails).
 *  Resolves to whether the write succeeded, for callers with extra UI state. */
export function useCopyToClipboard() {
  const dispatch = useAppDispatch();
  return useCallback((text: string, what: string): Promise<boolean> => (
    navigator.clipboard.writeText(text).then(
      () => {
        dispatch({ type: "SHOW_TRANSIENT_HINT", message: `Copied ${what}` });
        return true;
      },
      () => {
        dispatch({ type: "SHOW_TRANSIENT_HINT", message: `Couldn't copy ${what} to the clipboard` });
        return false;
      },
    )
  ), [dispatch]);
}
