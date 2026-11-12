// Language detection for built-in editor tabs. Imported only by
// `codemirror.ts`, so it stays in the lazily loaded editor chunk; each
// language's parser is a further lazy chunk loaded on first use.
import type { Extension } from "@codemirror/state";
import { LanguageDescription, StreamLanguage } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { markdown } from "@codemirror/lang-markdown";

/** Name-based gaps in `language-data`, mapped to one of its language names.
 *  `null` means plain text even if `language-data` would guess otherwise. */
function aliasFor(fileName: string): string | null | undefined {
  const name = fileName.toLowerCase();
  if (/^(gnu)?makefile$/.test(name) || name.endsWith(".mk")) return null;
  if (/^\.(bashrc|bash_profile|bash_logout|bash_aliases|zshrc|zshenv|zprofile|zlogin|profile|envrc)$/.test(name)) return "Shell";
  if (name.endsWith(".zsh")) return "Shell";
  if (name === ".env" || name.startsWith(".env.")) return "Properties files";
  if (/\.(jsonc|json5)$/.test(name) || /^(\.eslintrc|\.babelrc|\.prettierrc)$/.test(name)) return "JSON";
  if (name === "cargo.lock" || name === "poetry.lock" || name === "uv.lock") return "TOML";
  if (/^\.(git|docker|npm|prettier|eslint)ignore$/.test(name)) return null;
  return undefined;
}

/** Interpreter named on a `#!` line (`#!/bin/sh`, `#!/usr/bin/env -S python3 -u`). */
export function shebangInterpreter(firstLine: string): string | null {
  const match = /^#!\s*(\S+)(.*)$/.exec(firstLine);
  if (!match) return null;
  let program = match[1].split("/").pop() ?? "";
  if (program === "env") {
    const args = match[2].trim().split(/\s+/).filter((arg) => arg && !arg.startsWith("-") && !arg.includes("="));
    program = args[0] ?? "";
  }
  return program || null;
}

function languageForInterpreter(program: string): string | null {
  if (/^(sh|bash|zsh|dash|ksh|ash)$/.test(program)) return "Shell";
  if (/^python[0-9.]*$/.test(program)) return "Python";
  if (/^(node|nodejs|deno|bun)$/.test(program)) return "JavaScript";
  if (/^ruby[0-9.]*$/.test(program)) return "Ruby";
  if (/^perl[0-9.]*$/.test(program)) return "Perl";
  if (/^lua[0-9.]*$/.test(program)) return "Lua";
  return null;
}

function describe(fileName: string, firstLine: string): LanguageDescription | null {
  const alias = aliasFor(fileName);
  if (alias === null) return null;
  if (alias) return LanguageDescription.matchLanguageName(languages, alias, false);
  const byName = LanguageDescription.matchFilename(languages, fileName);
  if (byName) return byName;
  const program = shebangInterpreter(firstLine);
  const name = program ? languageForInterpreter(program) : null;
  return name ? LanguageDescription.matchLanguageName(languages, name, false) : null;
}

/** The language extension for a file, or `[]` for plain text. Never rejects:
 *  highlighting must not stop the editor from opening. */
export async function loadLanguage(fileName: string, firstLine: string): Promise<Extension> {
  try {
    if (/\.(md|markdown|mdx)$/i.test(fileName)) return markdown({ codeLanguages: languages });
    if (/\.(diff|patch)$/i.test(fileName)) {
      const { diff } = await import("@codemirror/legacy-modes/mode/diff");
      return StreamLanguage.define(diff);
    }
    const description = describe(fileName, firstLine);
    return description ? await description.load() : [];
  } catch (error) {
    console.warn(`[editor] could not load highlighting for ${fileName}:`, error);
    return [];
  }
}
