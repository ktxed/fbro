import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { COLOR_TOKENS } from "../theme/tokens";

interface ColorSchemePreviewProps {
  /** Token name (or `terminal.<key>`) to value; applied as inline custom properties. */
  values: Record<string, string>;
  mode: "dark" | "light";
  base: string;
  selectedToken: string | null;
  onSelectToken: (token: string) => void;
}

/** A miniature fbro drawn with the draft colours. Every element lists the
 *  tokens it shows in `data-tokens`; clicking one selects its token in the
 *  editor and the selected token's elements are outlined. */
export function ColorSchemePreview({ values, mode, base, selectedToken, onSelectToken }: ColorSchemePreviewProps) {
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    root.querySelectorAll("[data-token-active]").forEach((el) => el.removeAttribute("data-token-active"));
    if (!selectedToken) return;
    root.querySelectorAll(`[data-tokens~="${CSS.escape(selectedToken)}"]`).forEach((el) => el.setAttribute("data-token-active", ""));
  }, [selectedToken, values]);

  const style: Record<string, string> = { colorScheme: mode };
  for (const token of COLOR_TOKENS) {
    const value = values[token.name];
    if (value) style[`--${token.name}`] = value;
  }

  const pick = (event: React.MouseEvent) => {
    const el = (event.target as Element).closest("[data-tokens]");
    if (!el) return;
    const tokens = (el.getAttribute("data-tokens") ?? "").split(" ").filter(Boolean);
    if (tokens.length === 0) return;
    // Clicking the same element again cycles through the tokens it shows.
    const at = selectedToken ? tokens.indexOf(selectedToken) : -1;
    onSelectToken(tokens[(at + 1) % tokens.length]);
  };

  const t = (...tokens: string[]) => tokens.join(" ");
  const row = (tokens: string[], children: ReactNode, extra?: string) => (
    <div className={`csp__row${extra ? ` ${extra}` : ""}`} data-tokens={t(...tokens)} title={tokens.join(", ")}>
      {children}
    </div>
  );

  return (
    <div
      ref={rootRef}
      className="csp"
      data-testid="color-scheme-preview"
      data-color-scheme-base={base}
      style={style as CSSProperties}
      onClick={pick}
      role="img"
      aria-label="Live preview of the color scheme. Click an element to edit its color."
    >
      <div className="csp__panels" data-tokens="bg">
        <div className="csp__panel csp__panel--focused" data-tokens={t("bg-panel-focused", "border-focus")}>
          <div className="csp__tabs">
            <span className="csp__tab csp__tab--active" data-tokens={t("bg-tab-active", "text-path")}>~/work</span>
            <span className="csp__tab" data-tokens="bg-tab-focused-inactive">/etc</span>
          </div>
          <div className="csp__path" data-tokens={t("bg-header", "text-path")}>/home/you/work</div>
          {row(["text-dir"], <><span>..</span></>)}
          {row(["text-dir"], <><span>projects/</span><span className="csp__dim" data-tokens="text-size">&lt;dir&gt;</span></>)}
          {row(["text-link"], <><span>latest → v2</span></>)}
          {row(["text-archive"], <><span>backup.tar.gz</span><span className="csp__dim" data-tokens="text-size">1.2 GB</span></>)}
          {row(["text-exec"], <><span>build.sh</span><span className="csp__dim" data-tokens="text-size">3 KB</span></>)}
          {row(["text-media"], <><span>clip.mp4</span><span className="csp__dim" data-tokens="text-size">84 MB</span></>)}
          {row(["text-dot"], <><span>.bashrc</span></>)}
          {row(["text"], <><span>notes.txt</span><span className="csp__dim" data-tokens="text-size">2 KB</span></>)}
          {row(["bg-selected", "text-selected"], <><span>report.pdf</span><span>412 KB</span></>, "csp__row--selected")}
          {row(["bg-cursor-focused", "text-cursor"], <><span>cursor row</span></>, "csp__row--cursor")}
          {row(["bg-selected-cursor", "text-selected-cursor"], <><span>selected + cursor</span></>, "csp__row--selected-cursor")}
          <div className="csp__footer" data-tokens="bg-footer">
            <span data-tokens="text-dim">2 selected</span>
            <span data-tokens="text-remote">remote</span>
          </div>
        </div>
        <div className="csp__panel" data-tokens="bg-panel-unfocused">
          <div className="csp__tabs">
            <span className="csp__tab" data-tokens="bg-tab">/var</span>
          </div>
          <div className="csp__path" data-tokens={t("bg-header", "text-path")}>/var/log</div>
          {row(["text-dir"], <span>nginx/</span>)}
          {row(["bg-row-alt", "text"], <span>syslog</span>, "csp__row--alt")}
          {row(["bg-cursor-unfocused", "text-cursor-unfocused"], <span>auth.log</span>, "csp__row--cursor-unfocused")}
          {row(["bg-hover", "text"], <span>hover row</span>, "csp__row--hover")}
          {row(["bg-drop"], <span>drop target</span>, "csp__row--drop")}
          <div className="csp__footer" data-tokens="bg-footer">
            <span data-tokens="text-dim">0 selected</span>
          </div>
        </div>
      </div>

      <div className="csp__bar" data-tokens="bg-footer">
        <span className="csp__btn" data-tokens={t("bg-elev", "text", "border")}>F5 Copy</span>
        <span className="csp__btn" data-tokens={t("bg-elev", "text", "border")}>F8 Delete</span>
        <span className="csp__btn csp__btn--accent" data-tokens={t("accent", "text-on-accent")}>OK</span>
        <input className="csp__input" readOnly tabIndex={-1} value="filter" data-tokens={t("bg-input", "text", "border-strong")} aria-hidden="true" />
      </div>

      <div className="csp__overlay" data-tokens="bg-modal-overlay">
        <div className="csp__dialog" data-tokens={t("bg-panel", "border", "text")}>
          <div className="csp__dialog-title" data-tokens={t("bg-header", "text")}>Copy files</div>
          <div className="csp__dialog-body">
            <span data-tokens="text">Copy 2 files to /var?</span>
            <span data-tokens="text-muted">Muted hint text</span>
            <div className="csp__list-row" data-tokens={t("bg-cursor", "text")}>List cursor</div>
            <div className="csp__line"><span data-tokens="border-strong" className="csp__rule" /></div>
          </div>
        </div>
      </div>

      <div className="csp__status">
        <span data-tokens="text-warning">warning</span>
        <span data-tokens="text-danger">error</span>
        <span data-tokens="text-success">done</span>
        <span data-tokens="text-info">info</span>
        <span data-tokens="text-info-strong">info strong</span>
        <span data-tokens="text-highlight">match</span>
        <span data-tokens="text-paused">paused</span>
      </div>

      <div className="csp__toast" data-tokens={t("bg-toast", "border-toast", "text-toast")}>Copied full path</div>

      <div className="csp__terminal-wrap" data-tokens="bg-terminal">
        <div
          className="csp__terminal"
          data-tokens="terminal.background terminal.foreground"
          style={{ background: values["terminal.background"], color: values["terminal.foreground"] }}
        >
          <span>$ ls </span>
          <span data-tokens="terminal.selectionBackground" style={{ background: values["terminal.selectionBackground"] }}>selected</span>
          <span> </span>
          <span className="csp__cursor" data-tokens="terminal.cursor" style={{ background: values["terminal.cursor"] }}>&nbsp;</span>
          <span className="csp__chips">
            <span className="csp__chip" data-tokens={t("bg-terminal-chip", "text-terminal-chip")}>maximize</span>
            <span className="csp__chip csp__chip--hover" data-tokens={t("bg-terminal-chip-hover", "text-terminal-chip-hover")}>hover</span>
          </span>
        </div>
      </div>
    </div>
  );
}
