import type { PanelSlot } from "../ws/types";

/** A slow connection must not restore a previous tab order or closed tab. */
export function applySlotUpdate(
  existing: PanelSlot | undefined,
  slotId: string,
  version: number,
  patch: Partial<PanelSlot> & { remove?: boolean },
): PanelSlot | undefined {
  if (existing && version <= existing.version) return existing;
  if (patch.remove) return undefined;
  return {
    slotId,
    tabPanelIds: patch.tabPanelIds ?? existing?.tabPanelIds ?? [],
    activePanelId: patch.activePanelId ?? existing?.activePanelId ?? slotId,
    version,
  };
}
