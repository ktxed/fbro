// Colour strings accepted in theme files. Mirrors the backend validation in
// themes.rs (`validate_color`): hex and rgb()/rgba() only.
export const COLOR_RE = /^(?:#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})|rgba?\(\s*\d{1,3}(?:\.\d+)?\s*,\s*\d{1,3}(?:\.\d+)?\s*,\s*\d{1,3}(?:\.\d+)?\s*(?:,\s*(?:\d+(?:\.\d+)?|\.\d+)\s*)?\))$/;

export function isValidColor(value: string): boolean {
  return COLOR_RE.test(value.trim());
}

export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

export function parseColor(value: string): Rgba | null {
  const v = value.trim();
  if (!COLOR_RE.test(v)) return null;
  if (v.startsWith("#")) {
    const hex = v.slice(1);
    const full = hex.length <= 4 ? [...hex].map((c) => c + c).join("") : hex;
    const byte = (i: number) => parseInt(full.slice(i, i + 2), 16);
    return { r: byte(0), g: byte(2), b: byte(4), a: full.length === 8 ? Math.round((byte(6) / 255) * 100) / 100 : 1 };
  }
  const parts = v.slice(v.indexOf("(") + 1, -1).split(",").map((p) => parseFloat(p));
  return {
    r: clamp(Math.round(parts[0]), 0, 255),
    g: clamp(Math.round(parts[1]), 0, 255),
    b: clamp(Math.round(parts[2]), 0, 255),
    a: parts.length > 3 ? clamp(parts[3], 0, 1) : 1,
  };
}

const hex2 = (n: number) => n.toString(16).padStart(2, "0");

/** Format `rgba` in the style of `previous`: rgb()/rgba() stays functional,
 *  hex stays hex while opaque and becomes rgba() once it has transparency. */
export function formatColorLike(rgba: Rgba, previous: string): string {
  const a = Math.round(clamp(rgba.a, 0, 1) * 100) / 100;
  const functional = previous.trim().toLowerCase().startsWith("rgb");
  if (functional || a < 1) return `rgba(${rgba.r}, ${rgba.g}, ${rgba.b}, ${a})`;
  return `#${hex2(rgba.r)}${hex2(rgba.g)}${hex2(rgba.b)}`;
}
