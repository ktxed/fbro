import assert from "node:assert/strict";
import test from "node:test";
import { applySlotUpdate } from "../src/store/slots.ts";

test("delayed reorder and lifecycle patches cannot replace newer slot membership", () => {
  const current = { slotId: "left", tabPanelIds: ["b", "a"], activePanelId: "b", version: 3 };
  for (const version of [1, 2, 3]) {
    assert.strictEqual(applySlotUpdate(current, "left", version, {
      tabPanelIds: ["a", "b", "closed"], activePanelId: "closed",
    }), current);
    assert.strictEqual(applySlotUpdate(current, "left", version, { remove: true }), current);
  }
  const next = applySlotUpdate(current, "left", 4, { tabPanelIds: ["a", "b"] });
  assert.deepEqual(next, { slotId: "left", tabPanelIds: ["a", "b"], activePanelId: "b", version: 4 });
});

test("newer complete slot updates preserve activation even if predecessors arrive late", () => {
  const created = { tabPanelIds: ["a", "new"], activePanelId: "new" };
  const reordered = { tabPanelIds: ["new", "a"], activePanelId: "new" };
  const latest = applySlotUpdate(undefined, "left", 8, reordered);
  assert.deepEqual(latest, { slotId: "left", tabPanelIds: ["new", "a"], activePanelId: "new", version: 8 });
  assert.strictEqual(applySlotUpdate(latest, "left", 7, created), latest);
  assert.strictEqual(applySlotUpdate(latest, "left", 6, { ...created, version: 6 }), latest);
  assert.equal(applySlotUpdate(latest, "left", 9, { remove: true }), undefined);
});
