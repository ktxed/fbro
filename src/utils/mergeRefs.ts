import type { Ref } from "react";

/** Callback ref that forwards the node to every given ref. */
export function mergeRefs<T>(...refs: Array<Ref<T> | undefined>) {
  return (value: T | null) => {
    for (const ref of refs) {
      if (!ref) continue;
      if (typeof ref === "function") ref(value);
      else ref.current = value;
    }
  };
}
