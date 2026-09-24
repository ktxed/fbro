import { IconCopy } from "@tabler/icons-react";
import type { ButtonHTMLAttributes, ReactNode } from "react";

import { useCopyToClipboard } from "../hooks/useCopyToClipboard";

type CopyButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onClick" | "children"> & {
  /** Text written to the clipboard. */
  text: string;
  /** Lowercase noun for the toast: "Copied <what>". */
  what: string;
  iconSize?: number;
  /** Runs after the clipboard write settles, e.g. to hand focus back to a row. */
  onCopied?: (ok: boolean) => void;
  /** Optional visible label next to the icon; without it the button is icon-only. */
  children?: ReactNode;
};

/** The one way to render a copy-to-clipboard button: always toasts the result. */
export function CopyButton({
  text,
  what,
  iconSize = 14,
  onCopied,
  children,
  className = "modal__job-param-copy",
  title = `Copy ${what}`,
  type = "button",
  ...rest
}: CopyButtonProps) {
  const copyToClipboard = useCopyToClipboard();
  return (
    <button
      {...rest}
      type={type}
      className={className}
      title={title}
      aria-label={rest["aria-label"] ?? (children ? undefined : title)}
      onClick={() => {
        void copyToClipboard(text, what).then((ok) => onCopied?.(ok));
      }}
    >
      <IconCopy size={iconSize} aria-hidden />
      {children}
    </button>
  );
}
