import { type ColorSchemeCatalog } from "../theme/colorSchemes";
import { COLOR_RE } from "../theme/colorValue";

// The backend already rejects anything but hex and rgb()/rgba() colours; this
// keeps a malformed patch from reaching the stylesheet.
const KEY_RE = /^[a-z][a-z0-9-]{0,63}$/;
const ID_RE = /^custom:[a-z0-9][a-z0-9_-]{0,63}$/;

export function buildCustomSchemeCss(catalog: ColorSchemeCatalog | undefined): string {
  if (!catalog) return "";
  return catalog.items
    .filter((item) => ID_RE.test(item.id))
    .map((item) => {
      const decls = Object.entries(item.colors)
        .filter(([key, value]) => KEY_RE.test(key) && COLOR_RE.test(value.trim()))
        .map(([key, value]) => `--${key}:${value.trim()};`)
        .join("");
      return `[data-color-scheme="${item.id}"]{${decls}}`;
    })
    .join("\n");
}

/** Defines the custom schemes' tokens. Rendered after styles.css, so these
 *  rules win over the base scheme's block at equal specificity; tokens a scheme
 *  omits fall through to the base via `data-color-scheme-base`. */
export function CustomSchemeStyles({ catalog }: { catalog: ColorSchemeCatalog | undefined }) {
  const css = buildCustomSchemeCss(catalog);
  if (!css) return null;
  return <style data-fbro-custom-schemes="">{css}</style>;
}
