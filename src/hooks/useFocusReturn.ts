import { useCallback, useRef, type RefCallback } from "react";
import {
  captureFocusReturnTarget,
  registerFocusReturnTarget,
  returnPanelFocusOnRemoval,
  scheduleFocusReturn,
  type FocusReturnTarget,
} from "../utils/focusReturn";

/** Register when the DOM mounts, including portals mounted after their parent. */
export function useFocusReturn(panelId?: string): RefCallback<HTMLElement> {
  const targetRef = useRef<FocusReturnTarget | null>(null);
  return useCallback((owner: HTMLElement | null) => {
    if (!owner) return;
    const target = targetRef.current ?? captureFocusReturnTarget(panelId);
    targetRef.current = target;
    registerFocusReturnTarget(owner, target);
    return () => scheduleFocusReturn(target, owner);
  }, [panelId]);
}

/** Stable ref shared by controls whose removal returns input to a listing. */
export function usePanelFocusReturnOnRemoval(panelId: string): RefCallback<HTMLElement> {
  return useCallback((element: HTMLElement | null) => element
    ? returnPanelFocusOnRemoval(element, panelId) : undefined, [panelId]);
}
