import assert from "node:assert/strict";
import test from "node:test";
import { rovingIndexForKey } from "../src/components/modalKeyboard.ts";

test("bookmark arrows cycle through the current-folder action and saved bookmarks", () => {
  // Position zero is the current-folder action; saved bookmarks follow it.
  let position = 0;
  const positions = [];
  for (let repeat = 0; repeat < 5; repeat++) {
    position = rovingIndexForKey("ArrowUp", position, 4, "vertical");
    positions.push(position);
  }
  assert.deepEqual(positions, [3, 2, 1, 0, 3]);
  assert.equal(rovingIndexForKey("ArrowDown", 0, 4, "vertical"), 1);
  assert.equal(rovingIndexForKey("ArrowDown", 3, 4, "vertical"), 0);
});

test("Home and End include the current-folder action and last bookmark", () => {
  assert.equal(rovingIndexForKey("Home", 2, 4, "vertical"), 0);
  assert.equal(rovingIndexForKey("End", 0, 4, "vertical"), 3);
  for (const key of ["ArrowUp", "ArrowDown", "Home", "End"]) {
    assert.equal(rovingIndexForKey(key, 0, 1, "vertical"), 0);
  }
});

test("existing composite lists retain wrapping navigation by default", () => {
  assert.equal(rovingIndexForKey("ArrowUp", 0, 3, "vertical"), 2);
  assert.equal(rovingIndexForKey("ArrowDown", 2, 3, "vertical"), 0);
  assert.equal(rovingIndexForKey("ArrowLeft", 0, 3, "horizontal"), 2);
  assert.equal(rovingIndexForKey("ArrowRight", 2, 3, "horizontal"), 0);
  assert.equal(rovingIndexForKey("Enter", 0, 3, "vertical"), null);
  assert.equal(rovingIndexForKey("ArrowUp", 0, 0, "vertical"), null);
});
