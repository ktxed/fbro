import { readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

export function cargoPackageVersion(toml) {
  const header = toml.split(/\r?\n\[/)[0];
  const match = header.match(/^version\s*=\s*"([^"]+)"/m);
  return match ? match[1] : null;
}

export function readVersions(rootDir) {
  const pkg = JSON.parse(readFileSync(path.join(rootDir, "package.json"), "utf8"));
  const tauri = JSON.parse(readFileSync(path.join(rootDir, "src-tauri", "tauri.conf.json"), "utf8"));
  const cargo = cargoPackageVersion(readFileSync(path.join(rootDir, "src-tauri", "Cargo.toml"), "utf8"));
  return {
    package: typeof pkg.version === "string" ? pkg.version : null,
    tauri: typeof tauri.version === "string" ? tauri.version : null,
    cargo,
  };
}

export function versionErrors(versions, tag) {
  const errors = [];
  const values = [versions.package, versions.tauri, versions.cargo];
  if (values.some((value) => !value)) {
    errors.push(`missing version (package=${versions.package}, tauri=${versions.tauri}, cargo=${versions.cargo})`);
  } else if (new Set(values).size !== 1) {
    errors.push(`versions differ: package=${versions.package} tauri=${versions.tauri} cargo=${versions.cargo}`);
  }
  if (tag && versions.tauri && tag !== versions.tauri) {
    errors.push(`tag ${tag} does not match tauri.conf.json version ${versions.tauri}`);
  }
  return errors;
}

function parseTag(argv) {
  const index = argv.indexOf("--tag");
  if (index === -1) return null;
  const tag = argv[index + 1];
  if (!tag) throw new Error("--tag requires a value");
  return tag;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const errors = versionErrors(readVersions(process.cwd()), parseTag(process.argv.slice(2)));
    if (errors.length > 0) {
      for (const error of errors) console.error(error);
      process.exit(1);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
