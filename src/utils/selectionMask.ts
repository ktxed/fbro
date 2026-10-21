import type { VFSEntry } from "../ws/types";

/** Prompt default only; matching and selection resolution belong to the backend. */
export function selectionMaskForEntry(entry?: Pick<VFSEntry, "name" | "kind" | "targetKind">): string {
  if (!entry || entry.kind === "directory" || entry.targetKind === "directory") return "*.*";
  const dot = entry.name.lastIndexOf(".");
  return dot > 0 && dot < entry.name.length - 1 ? `*${entry.name.slice(dot)}` : "*.*";
}
