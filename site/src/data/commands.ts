// Default key bindings, generated from the backend command registry
// (src-tauri/src/ws/commands.rs). Regenerate with:
//   FBRO_UPDATE_SITE_DATA=1 cargo test --manifest-path src-tauri/Cargo.toml site_command_data_is_current
import data from "./commands.json";

export type Platform = "linux" | "macos" | "windows";

export interface KeyStroke {
  key: string;
  shift?: boolean;
  ctrl?: boolean;
  alt?: boolean;
  meta?: boolean;
}

export interface KeyBinding {
  strokes: KeyStroke[];
  modes: string[];
}

export interface Command {
  id: string;
  category: string;
  title: string;
  bindings: KeyBinding[];
}

export const PLATFORMS: { id: Platform; label: string }[] = [
  { id: "linux", label: "Linux" },
  { id: "windows", label: "Windows" },
  { id: "macos", label: "macOS" },
];

const commands = data.platforms as Record<Platform, Command[]>;

export function commandsFor(platform: Platform): Command[] {
  return commands[platform];
}

export function findCommand(platform: Platform, id: string): Command | undefined {
  return commands[platform].find((c) => c.id === id);
}

// Same labels as src/commands/bindings.ts `formatBinding`, so the guide reads
// exactly like the app's menus and cheat sheet.
const KEY_LABELS: Record<string, string> = {
  " ": "Space",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
};

/** Key caps of one stroke, e.g. ["Ctrl", "Shift", "P"]. */
export function strokeKeys(stroke: KeyStroke, platform: Platform): string[] {
  const parts: string[] = [];
  if (stroke.meta) parts.push(platform === "macos" ? "Cmd" : "Meta");
  if (stroke.ctrl) parts.push("Ctrl");
  if (stroke.alt) parts.push(platform === "macos" ? "Option" : "Alt");
  if (stroke.shift) parts.push("Shift");
  parts.push(KEY_LABELS[stroke.key] ?? (stroke.key.length === 1 ? stroke.key.toUpperCase() : stroke.key));
  return parts;
}

export function modeNote(binding: KeyBinding): string | undefined {
  if (binding.modes.length === 1) return `${binding.modes[0]} view`;
  return undefined;
}

/** Category order used by the reference page; unknown categories go last. */
export const CATEGORY_ORDER = ["Navigation", "Selection", "File", "File Operation", "Panel", "View", "Git", "Notes", "App"];
