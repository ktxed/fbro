import assert from "node:assert/strict";
import test from "node:test";
import {
  createNotificationState,
  applyNotificationSnapshot,
  applyNotificationPatch,
  expireNotificationToast,
} from "../src/notifications/state.ts";

function entry(id, channels = ["toast", "log"]) {
  return { id, timestamp: 1720000000000, message: "Repeated message", level: "info", channels };
}

test("live notifications queue every toast in arrival order and preserve repeated text", () => {
  let state = createNotificationState();
  for (const [index, id] of ["first", "second", "third"].entries()) {
    state = applyNotificationPatch(state, index + 1, { added: entry(id), removedIds: [] });
  }
  assert.deepEqual(state.entries.map((item) => item.id), ["third", "second", "first"]);
  assert.deepEqual(state.toastQueue.map((item) => item.id), ["first", "second", "third"]);
  const next = expireNotificationToast(state, "first");
  assert.deepEqual(next.toastQueue.map((item) => item.id), ["second", "third"]);
  assert.equal(next.entries.length, 3);
});

test("snapshots restore history without replaying toasts or disturbing queued delivery", () => {
  const restored = applyNotificationSnapshot(createNotificationState(), { version: 7, entries: [entry("old")] });
  assert.equal(restored.entries[0].id, "old");
  assert.deepEqual(restored.toastQueue, []);
  const live = applyNotificationPatch(restored, 8, { added: entry("live"), removedIds: [] });
  const resynced = applyNotificationSnapshot(live, { version: 8, entries: [entry("live"), entry("old")] });
  assert.deepEqual(resynced.toastQueue.map((item) => item.id), ["live"]);
});

test("stale and duplicate patches never repeat history or toast delivery", () => {
  const state = applyNotificationPatch(createNotificationState(), 3, { added: entry("latest"), removedIds: [] });
  assert.strictEqual(applyNotificationPatch(state, 3, { added: entry("latest"), removedIds: [] }), state);
  assert.strictEqual(applyNotificationPatch(state, 2, { clear: true }), state);
  assert.strictEqual(applyNotificationSnapshot(state, { version: 2, entries: [] }), state);
});

test("channel selection records log-only and history-only messages without toasts", () => {
  let state = applyNotificationPatch(createNotificationState(), 1, { added: entry("log", ["log"]), removedIds: [] });
  state = applyNotificationPatch(state, 2, { added: entry("history", []), removedIds: [] });
  state = applyNotificationPatch(state, 3, { added: entry("toast", ["toast"]), removedIds: [] });
  assert.deepEqual(state.entries.map((item) => item.id), ["toast", "history", "log"]);
  assert.deepEqual(state.toastQueue.map((item) => item.id), ["toast"]);
});

test("eviction and clear affect history while every pending toast still delivers", () => {
  let state = applyNotificationPatch(createNotificationState(), 1, { added: entry("first"), removedIds: [] });
  state = applyNotificationPatch(state, 2, { added: entry("second"), removedIds: ["first"] });
  assert.deepEqual(state.entries.map((item) => item.id), ["second"]);
  state = applyNotificationPatch(state, 3, { clear: true });
  assert.deepEqual(state.entries, []);
  assert.deepEqual(state.toastQueue.map((item) => item.id), ["first", "second"]);
});

test("an old expiry cannot dismiss a newer active toast", () => {
  let state = applyNotificationPatch(createNotificationState(), 1, { added: entry("first"), removedIds: [] });
  state = applyNotificationPatch(state, 2, { added: entry("second"), removedIds: [] });
  const next = expireNotificationToast(state, "first");
  assert.strictEqual(expireNotificationToast(next, "first"), next);
  assert.strictEqual(expireNotificationToast(state, "second"), state);
});
