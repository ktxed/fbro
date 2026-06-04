export type ColorSchemeId = "fbro-dark" | "daylight";

export interface ColorScheme {
  id: ColorSchemeId;
  label: string;
  description: string;
  mode: "dark" | "light";
  terminal: {
    background: string;
    foreground: string;
    cursor: string;
    selectionBackground: string;
  };
  mermaidTheme: "dark" | "default";
}

export const COLOR_SCHEMES: ColorScheme[] = [
  {
    id: "fbro-dark",
    label: "fbro Dark",
    description: "Original dark blue scheme",
    mode: "dark",
    terminal: {
      background: "#1e1e1e",
      foreground: "#dddddd",
      cursor: "#d6e8ff",
      selectionBackground: "#2d228a",
    },
    mermaidTheme: "dark",
  },
  {
    id: "daylight",
    label: "Daylight",
    description: "Light neutral scheme",
    mode: "light",
    terminal: {
      background: "#ffffff",
      foreground: "#1e2933",
      cursor: "#2f6fb3",
      selectionBackground: "#cfe3ff",
    },
    mermaidTheme: "default",
  },
];

export const DEFAULT_COLOR_SCHEME_ID: ColorSchemeId = "fbro-dark";

export function resolveColorScheme(id: string | undefined): ColorScheme {
  return COLOR_SCHEMES.find((scheme) => scheme.id === id) ?? COLOR_SCHEMES[0];
}
