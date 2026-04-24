export function isArchiveSourceId(sourceId: string): boolean {
  return sourceId.startsWith("zip:") || sourceId.startsWith("7z:");
}

export function isSearchSourceId(sourceId: string): boolean {
  return sourceId.startsWith("search:");
}

/** Strips the "zip:" or "7z:" prefix and returns the filesystem path to the archive. */
export function archiveFilePath(sourceId: string): string {
  if (sourceId.startsWith("zip:")) return sourceId.slice(4);
  if (sourceId.startsWith("7z:")) return sourceId.slice(3);
  return sourceId;
}
