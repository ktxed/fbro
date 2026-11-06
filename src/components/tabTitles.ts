// Shared tab-chrome helpers for the tab strip (`PanelTabBar`) and the
// searchable tab list (`PanelTabList`), so both name a tab the same way and
// closing an editor tab always flushes it first.
import type { WsClient } from "../ws/client";
import type { PanelSlot, PanelState, Source } from "../ws/types";
import { flushEditor } from "../editor/sessions";
import { isSearchSourceId } from "../utils/sourceId";

export function basename(path: string): string {
  if (!path) return "";
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  if (!normalized) return "/";
  const i = normalized.lastIndexOf("/");
  return i < 0 ? normalized : normalized.slice(i + 1);
}

/** Name of a virtual source that stands in for a folder title. */
export function sourceTabTitle(
  panel: PanelState,
  sources: Record<string, Source>,
): string | undefined {
  if (isSearchSourceId(panel.sourceId)) return panel.sourceId;
  if (panel.sourceId === "procs" || panel.sourceId === "journal") {
    return sources[panel.sourceId]?.name ?? panel.sourceId;
  }
  return undefined;
}

/** Bold part of a tab: the tab's own title, or one derived from it. */
export function tabTitle(
  panel: PanelState | undefined,
  sources: Record<string, Source>,
): string {
  if (!panel) return "…";
  if (panel.tabTitle) return panel.tabTitle;
  if (panel.editor) return panel.editor.fileName;
  if (panel.tabKind === "terminal") {
    const base = basename(panel.path) || "~";
    return base;
  }
  const sourceTitle = sourceTabTitle(panel, sources);
  if (sourceTitle) return sourceTitle;
  return basename(panel.path) || "/";
}

/** Secondary line of a tab row: the edited file, the folder, or the source. */
export function tabDetail(
  panel: PanelState | undefined,
  sources: Record<string, Source>,
): string {
  if (!panel) return "";
  if (panel.editor) return panel.editor.filePath;
  if (panel.path) return panel.path;
  return sources[panel.sourceId]?.name ?? panel.sourceId;
}

/**
 * Keep the end of a path visible. CSS would ellipsize the tail, which is the
 * part users recognise, so trim the front here and let CSS clip whatever is
 * still too wide for the row.
 */
export function keepTail(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text;
  return `…${text.slice(text.length - maxChars + 1)}`;
}

/**
 * Close a tab. An editor tab saves before it closes; the last tab in a slot
 * cannot be closed.
 */
export function requestCloseTab(
  slot: PanelSlot,
  panels: Record<string, PanelState>,
  client: WsClient,
  panelId: string,
): void {
  if (slot.tabPanelIds.length <= 1) return;
  const editor = panels[panelId]?.editor;
  if (editor) void flushEditor(editor.sessionId).then(() => client.sendIntent("panel.tab.close", undefined, { panelId }));
  else client.sendIntent("panel.tab.close", undefined, { panelId });
}
