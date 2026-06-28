import type { WheelEvent } from "react";

export function stepNumberInputOnWheel(event: WheelEvent<HTMLInputElement>) {
  if (event.currentTarget.disabled || event.deltaY === 0) return;
  event.preventDefault();

  const input = event.currentTarget;
  const direction = event.deltaY < 0 ? 1 : -1;
  try {
    if (direction > 0) {
      input.stepUp();
    } else {
      input.stepDown();
    }
  } catch {
    return;
  }
  input.dispatchEvent(new Event("input", { bubbles: true }));
}
