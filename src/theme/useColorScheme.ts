import { useAppState } from "../store";
import { resolveColorScheme, type ColorScheme } from "./colorSchemes";

/** The scheme every client renders right now (backend-resolved). */
export function useColorScheme(): ColorScheme {
  const { activeColorScheme, colorSchemes } = useAppState().settings.general;
  return resolveColorScheme(activeColorScheme, colorSchemes);
}
