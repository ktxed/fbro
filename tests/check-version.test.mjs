import assert from "node:assert/strict";
import test from "node:test";

import { cargoPackageVersion, versionErrors } from "../scripts/check-version.mjs";

test("reads the package version and ignores dependency versions", () => {
  const toml = [
    "[package]",
    'name = "fbro"',
    'version = "1.2.3"',
    "",
    "[dependencies]",
    'serde = { version = "1" }',
    "",
  ].join("\n");
  assert.equal(cargoPackageVersion(toml), "1.2.3");
});

test("accepts three matching versions and the same tag", () => {
  const versions = { package: "0.0.1", tauri: "0.0.1", cargo: "0.0.1" };
  assert.deepEqual(versionErrors(versions, null), []);
  assert.deepEqual(versionErrors(versions, "0.0.1"), []);
});

test("rejects a v-prefixed tag", () => {
  const versions = { package: "0.0.1", tauri: "0.0.1", cargo: "0.0.1" };
  assert.match(versionErrors(versions, "v0.0.1").join("\n"), /tag v0\.0\.1/);
});

test("reports mismatched files and a tag that disagrees with tauri.conf.json", () => {
  const versions = { package: "0.0.1", tauri: "0.0.2", cargo: "0.0.1" };
  assert.match(versionErrors(versions, "0.0.3").join("\n"), /versions differ/);
  assert.match(versionErrors(versions, "0.0.3").join("\n"), /tag 0\.0\.3/);
});

test("reports a missing version", () => {
  assert.match(versionErrors({ package: "0.0.1", tauri: "0.0.1", cargo: null }, null).join("\n"), /missing version/);
});
