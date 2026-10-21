import assert from "node:assert/strict";
import test from "node:test";
import { selectionMaskForEntry } from "../src/utils/selectionMask.ts";

test("mask prefill uses the final extension and preserves its spelling", () => {
  for (const [name, expected] of [["notes.txt", "*.txt"], ["PHOTO.JPG", "*.JPG"], ["archive.tar.gz", "*.gz"], [".config.json", "*.json"]]) {
    assert.equal(selectionMaskForEntry({ name, kind: "file" }), expected);
  }
});

test("mask prefill falls back to all files for folders and extensionless names", () => {
  for (const name of ["README", ".bashrc", "trailing."]) {
    assert.equal(selectionMaskForEntry({ name, kind: "file" }), "*.*");
  }
  assert.equal(selectionMaskForEntry({ name: "folder.txt", kind: "directory" }), "*.*");
  assert.equal(selectionMaskForEntry({ name: "folder.txt", kind: "symlink", targetKind: "directory" }), "*.*");
  assert.equal(selectionMaskForEntry(undefined), "*.*");
  assert.equal(selectionMaskForEntry({ name: "file.txt", kind: "symlink", targetKind: "file" }), "*.txt");
});
