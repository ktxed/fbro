export const DARK_SCHEME_ID = "fbro-dark";
export const LIGHT_SCHEME_ID = "daylight";
export const CUSTOM_SCHEME_PREFIX = "custom:";

/** Built-in ids are fixed; user files are `custom:<file stem>`; schemes shipped
 *  inside fbro are `bundled:<name>`. */
export type ColorSchemeId = string;
export type BuiltinColorSchemeId = typeof DARK_SCHEME_ID | typeof LIGHT_SCHEME_ID;

export interface TerminalColors {
  background: string;
  foreground: string;
  cursor: string;
  selectionBackground: string;
}

export interface ColorScheme {
  id: ColorSchemeId;
  label: string;
  description: string;
  mode: "dark" | "light";
  /** Built-in scheme whose stylesheet block supplies tokens a custom scheme omits. */
  base: BuiltinColorSchemeId;
  terminal: TerminalColors;
  mermaidTheme: "dark" | "default";
  /** A user file in the themes folder: the only kind the editor can change. */
  custom: boolean;
  /** Shipped inside fbro: read-only, duplicate to edit. */
  bundled: boolean;
}

/** A custom scheme as projected by the backend (`general.colorSchemes.items`). */
export interface CustomColorScheme {
  id: string;
  name: string;
  description: string;
  mode: "dark" | "light";
  base: BuiltinColorSchemeId;
  bundled?: boolean;
  /** CSS token name (without `--`) to colour. May be partial. */
  colors: Record<string, string>;
  /** Already resolved against the base by the backend. */
  terminal: TerminalColors;
}

export interface ColorSchemeCatalog {
  dir: string;
  items: CustomColorScheme[];
  errors: { file: string; message: string }[];
}

export const EMPTY_COLOR_SCHEME_CATALOG: ColorSchemeCatalog = { dir: "", items: [], errors: [] };

export const COLOR_SCHEMES: ColorScheme[] = [
  {
    id: DARK_SCHEME_ID,
    label: "fbro Dark",
    description: "Original dark blue scheme",
    mode: "dark",
    base: DARK_SCHEME_ID,
    terminal: {
      background: "#1e1e1e",
      foreground: "#dddddd",
      cursor: "#d6e8ff",
      selectionBackground: "#2d228a",
    },
    mermaidTheme: "dark",
    custom: false,
    bundled: false,
  },
  {
    id: LIGHT_SCHEME_ID,
    label: "fbro Light",
    description: "Light neutral scheme",
    mode: "light",
    base: LIGHT_SCHEME_ID,
    terminal: {
      background: "#ffffff",
      foreground: "#1e2933",
      cursor: "#2f6fb3",
      selectionBackground: "#cfe3ff",
    },
    mermaidTheme: "default",
    custom: false,
    bundled: false,
  },
];

export const DEFAULT_COLOR_SCHEME_ID: ColorSchemeId = DARK_SCHEME_ID;

function fromCustom(item: CustomColorScheme): ColorScheme {
  return {
    id: item.id,
    label: item.name,
    description: item.description || `${item.bundled ? "Bundled" : "Custom"} scheme based on ${item.base === LIGHT_SCHEME_ID ? "fbro Light" : "fbro Dark"}`,
    mode: item.mode,
    base: item.base,
    terminal: item.terminal,
    mermaidTheme: item.mode === "dark" ? "dark" : "default",
    custom: !item.bundled,
    bundled: item.bundled === true,
  };
}

/** Built-ins first, then the custom schemes the backend loaded. */
export function listColorSchemes(catalog?: ColorSchemeCatalog): ColorScheme[] {
  return [...COLOR_SCHEMES, ...(catalog?.items ?? []).map(fromCustom)];
}

/** Unknown or unloaded ids fall back to fbro Dark, like the backend. */
export function resolveColorScheme(id: string | undefined | null, catalog?: ColorSchemeCatalog): ColorScheme {
  const builtin = COLOR_SCHEMES.find((scheme) => scheme.id === id);
  if (builtin) return builtin;
  const custom = catalog?.items.find((item) => item.id === id);
  return custom ? fromCustom(custom) : COLOR_SCHEMES[0];
}

/** Attributes that make an element a scheme root (portals set them too). */
export function colorSchemeAttrs(scheme: ColorScheme) {
  return {
    "data-color-scheme": scheme.id,
    "data-color-scheme-base": scheme.base,
  } as const;
}
