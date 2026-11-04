import type { ReactNode } from "react";
import {
  IconArrowBigUp, IconArrowDown, IconArrowLeft, IconArrowRight, IconArrowUp,
  IconBackspace, IconChevronUp, IconCommand, IconOption,
} from "@tabler/icons-react";

import { formatBinding, strokeTokens, type KeyToken } from "../commands/bindings";
import { isMac } from "../utils/platform";
import type { KeyBinding } from "../ws/types";

const ICON_SIZE = 14;

const MAC_MODIFIER_ICONS = {
  meta: IconCommand,
  alt: IconOption,
  shift: IconArrowBigUp,
  ctrl: IconChevronUp,
} as const;

const MODIFIER_LABELS = { meta: isMac ? "Cmd" : "Meta", ctrl: "Ctrl", alt: "Alt", shift: "Shift" } as const;

const ARROW_ICONS: Record<string, typeof IconArrowUp> = {
  ArrowUp: IconArrowUp,
  ArrowDown: IconArrowDown,
  ArrowLeft: IconArrowLeft,
  ArrowRight: IconArrowRight,
};

const KEY_LABELS: Record<string, string> = { " ": "Space", PageUp: "PgUp", PageDown: "PgDn" };

function tokenContent(token: KeyToken): ReactNode {
  if (token.kind === "modifier") {
    if (!isMac) return MODIFIER_LABELS[token.modifier];
    const Icon = MAC_MODIFIER_ICONS[token.modifier];
    return <Icon size={ICON_SIZE} stroke={1.75} aria-hidden="true" />;
  }
  const Arrow = ARROW_ICONS[token.key];
  if (Arrow) return <Arrow size={ICON_SIZE} stroke={1.75} aria-hidden="true" />;
  // The Mac key labelled "delete" sends Backspace.
  if (isMac && token.key === "Backspace") return <IconBackspace size={ICON_SIZE} stroke={1.75} aria-hidden="true" />;
  if (isMac && token.key === "Enter") return "Return";
  return KEY_LABELS[token.key] ?? (token.key.length === 1 ? token.key.toUpperCase() : token.key);
}

/** Key caps for one binding. macOS shows the ⌘ ⌥ ⇧ ⌃ symbols; the accessible
 *  name is always the spelled-out `formatBinding` text. */
export function KeyCombo({ binding }: { binding: KeyBinding }) {
  const label = formatBinding(binding);
  return (
    <span className={`key-combo${isMac ? " key-combo--mac" : ""}`} role="img" aria-label={label} title={label}>
      {binding.strokes.map((stroke, strokeIndex) => (
        <span key={strokeIndex} className="key-combo__stroke">
          {strokeTokens(stroke).map((token, index) => (
            <kbd
              key={index}
              className={`key-combo__key${token.kind === "modifier" ? " key-combo__key--modifier" : ""}`}
              aria-hidden="true"
            >
              {tokenContent(token)}
            </kbd>
          ))}
        </span>
      ))}
    </span>
  );
}
