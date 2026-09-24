import { useCallback } from "react";
import { useAppDispatch } from "../store";

/** Returns `copy(text, what)`: writes `text` to the system clipboard and
 *  shows a "Copied <what>" toast (or a failure toast if the write fails). */
export function useCopyToClipboard() {
  const dispatch = useAppDispatch();
  return useCallback((text: string, what: string) => {
    void navigator.clipboard.writeText(text).then(
      () => dispatch({ type: "SHOW_TRANSIENT_HINT", message: `Copied ${what}` }),
      () => dispatch({ type: "SHOW_TRANSIENT_HINT", message: `Couldn't copy ${what} to the clipboard` }),
    );
  }, [dispatch]);
}
