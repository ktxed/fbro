/** Registry of editable colour tokens: the editor, the template capture
 *  (Duplicate) and the preview all read it. A new `--token` in styles.css that
 *  is a colour needs an entry here. Non-colour tokens (font, sizes,
 *  --shadow-modal) are deliberately excluded. */

export type TokenGroupId = "surfaces" | "rows" | "files" | "borders" | "status" | "toasts" | "terminal";

export interface TokenGroup {
  id: TokenGroupId;
  label: string;
}

export const TOKEN_GROUPS: TokenGroup[] = [
  { id: "surfaces", label: "Surfaces" },
  { id: "rows", label: "Panel rows & cursor" },
  { id: "files", label: "File types" },
  { id: "borders", label: "Borders & accent" },
  { id: "status", label: "Status & feedback" },
  { id: "toasts", label: "Toasts" },
  { id: "terminal", label: "Terminal" },
];

export interface ColorToken {
  /** CSS custom property without the leading `--`. */
  name: string;
  label: string;
  group: TokenGroupId;
  /** Where the colour shows up. */
  description: string;
}

export const COLOR_TOKENS: ColorToken[] = [
  // Surfaces
  { name: "bg", group: "surfaces", label: "App background", description: "Window background behind the panels" },
  { name: "bg-panel", group: "surfaces", label: "Panel", description: "Base panel and dialog body background" },
  { name: "bg-panel-unfocused", group: "surfaces", label: "Panel (unfocused)", description: "Background of the panel that does not have focus" },
  { name: "bg-panel-focused", group: "surfaces", label: "Panel (focused)", description: "Background of the focused panel" },
  { name: "bg-header", group: "surfaces", label: "Header", description: "Panel path bar and dialog headers" },
  { name: "bg-footer", group: "surfaces", label: "Footer", description: "Panel status line and button bar" },
  { name: "bg-input", group: "surfaces", label: "Input", description: "Text fields and filter boxes" },
  { name: "bg-elev", group: "surfaces", label: "Raised surface", description: "Menus, popovers and buttons" },
  { name: "bg-focused", group: "surfaces", label: "Focused highlight", description: "Focused areas, drag source, active highlights" },
  { name: "bg-hover", group: "surfaces", label: "Hover", description: "Row and control hover" },
  { name: "bg-row-alt", group: "surfaces", label: "Alternate row", description: "Zebra striping in lists" },
  { name: "bg-drop", group: "surfaces", label: "Drop target", description: "Highlight while dragging files over a panel" },
  { name: "bg-modal-overlay", group: "surfaces", label: "Dialog overlay", description: "Dim layer behind dialogs" },
  { name: "bg-tab", group: "surfaces", label: "Tab", description: "Inactive panel tab" },
  { name: "bg-tab-focused-inactive", group: "surfaces", label: "Tab (focused panel)", description: "Inactive tab in the focused panel" },
  { name: "bg-tab-active", group: "surfaces", label: "Active tab", description: "Selected panel tab" },
  // Rows & cursor
  { name: "text", group: "rows", label: "Text", description: "Default file names and body text" },
  { name: "bg-cursor", group: "rows", label: "Cursor row", description: "Cursor row background in list views" },
  { name: "bg-cursor-focused", group: "rows", label: "Cursor (focused panel)", description: "Cursor row in the focused panel" },
  { name: "bg-cursor-unfocused", group: "rows", label: "Cursor (unfocused panel)", description: "Cursor row in the unfocused panel" },
  { name: "text-cursor", group: "rows", label: "Cursor text", description: "Text on the focused cursor row" },
  { name: "text-cursor-unfocused", group: "rows", label: "Cursor text (unfocused)", description: "Text on the unfocused cursor row" },
  { name: "bg-selected", group: "rows", label: "Selected row", description: "Background of selected files" },
  { name: "text-selected", group: "rows", label: "Selected text", description: "Text of selected files" },
  { name: "bg-selected-cursor", group: "rows", label: "Selected + cursor row", description: "Cursor on a selected file" },
  { name: "text-selected-cursor", group: "rows", label: "Selected + cursor text", description: "Text of the cursor on a selected file" },
  // File types
  { name: "text-dir", group: "files", label: "Folder", description: "Directory names" },
  { name: "text-link", group: "files", label: "Symlink", description: "Symbolic links" },
  { name: "text-archive", group: "files", label: "Archive", description: "Archives such as zip, tar, 7z" },
  { name: "text-exec", group: "files", label: "Executable", description: "Executable files" },
  { name: "text-media", group: "files", label: "Media", description: "Images, audio and video" },
  { name: "text-dot", group: "files", label: "Hidden file", description: "Dotfiles when hidden files are shown" },
  { name: "text-size", group: "files", label: "Size and date", description: "Size and date columns" },
  { name: "text-path", group: "files", label: "Path", description: "Path bar text and active tab title" },
  { name: "text-remote", group: "files", label: "Remote", description: "Remote fbro and network locations" },
  // Borders & accent
  { name: "border", group: "borders", label: "Border", description: "Panel, dialog and control borders" },
  { name: "border-focus", group: "borders", label: "Focus border", description: "Border of the focused panel and active controls" },
  { name: "border-strong", group: "borders", label: "Strong border", description: "Separators that need more contrast" },
  { name: "accent", group: "borders", label: "Accent", description: "Primary buttons, progress and checked controls" },
  { name: "text-on-accent", group: "borders", label: "Text on accent", description: "Text drawn on the accent colour" },
  { name: "text-dim", group: "borders", label: "Dim text", description: "Secondary labels and hints" },
  { name: "text-muted", group: "borders", label: "Muted text", description: "Disabled and placeholder text" },
  // Status
  { name: "text-warning", group: "status", label: "Warning", description: "Warnings and cautions" },
  { name: "text-danger", group: "status", label: "Danger", description: "Errors and destructive actions" },
  { name: "text-success", group: "status", label: "Success", description: "Completed jobs and added lines" },
  { name: "text-info", group: "status", label: "Info", description: "Informational messages" },
  { name: "text-info-strong", group: "status", label: "Info (strong)", description: "Emphasised informational text" },
  { name: "text-highlight", group: "status", label: "Highlight", description: "Search matches and emphasis" },
  { name: "text-paused", group: "status", label: "Paused", description: "Paused jobs" },
  // Toasts
  { name: "bg-toast", group: "toasts", label: "Toast", description: "Toast background" },
  { name: "border-toast", group: "toasts", label: "Toast border", description: "Toast outline" },
  { name: "text-toast", group: "toasts", label: "Toast text", description: "Toast message" },
  // Terminal chips (the terminal's own palette is below)
  { name: "bg-terminal", group: "terminal", label: "Terminal panel", description: "Panel background behind an embedded terminal" },
  { name: "bg-terminal-chip", group: "terminal", label: "Terminal chip", description: "Floating chips over the terminal" },
  { name: "bg-terminal-chip-hover", group: "terminal", label: "Terminal chip (hover)", description: "Chip hover state" },
  { name: "text-terminal-chip", group: "terminal", label: "Terminal chip text", description: "Chip label" },
  { name: "text-terminal-chip-hover", group: "terminal", label: "Terminal chip text (hover)", description: "Chip label on hover" },
];

export type TerminalKey = "background" | "foreground" | "cursor" | "selectionBackground";

export interface TerminalToken {
  key: TerminalKey;
  label: string;
  description: string;
}

/** The embedded terminal's palette lives beside the CSS tokens in the theme file. */
export const TERMINAL_TOKENS: TerminalToken[] = [
  { key: "background", label: "Terminal background", description: "Background of the embedded terminal" },
  { key: "foreground", label: "Terminal text", description: "Default terminal text" },
  { key: "cursor", label: "Terminal cursor", description: "Terminal cursor" },
  { key: "selectionBackground", label: "Terminal selection", description: "Selected text in the terminal" },
];

/** Snapshot of the colours rendered under `el`: the template for a new scheme. */
export function captureRenderedTokens(el: Element): Record<string, string> {
  const style = getComputedStyle(el);
  const colors: Record<string, string> = {};
  for (const token of COLOR_TOKENS) {
    const value = style.getPropertyValue(`--${token.name}`).trim();
    if (value) colors[token.name] = value;
  }
  return colors;
}
