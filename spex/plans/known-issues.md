# Known issues and quirks

Bugs, rough edges and spec drift found while writing the user guide (`site/`),
collected so they can be revisited. Each entry records what was observed, where,
and a possible fix. Line numbers are as of 2026-11-04 (`development`, 64dda2c).

When an issue is fixed, remove its entry, and update the user-guide page that
mentions it.

| # | Issue | Kind | Severity |
|---|---|---|---|
| 1 | Delete is permanent, with no trash | Missing feature | Medium |
| 2 | Escape never reaches programs in the embedded terminal | Quirk | Medium |
| 3 | `thumbnails.md` says grid view is Shift+G | Spec drift | Low |
| 4 | Two registry commands are titled "Open Entry" | Quirk | Low |
| 5 | AGENTS.md says remote fbro uses system ssh | Doc drift | Low |
| 6 | `bro.md` §5.2 lists an outdated set of bros | Spec drift | Low |

---

## 1. Delete is permanent, with no trash

**Observed.** Delete (F8 / macOS 8) removes files directly. Nothing in
`src-tauri/src` or `src` uses the system trash or recycle bin.

**Where.** `src-tauri/src/vfs/local.rs:333` `remove_file` and `:337`
`remove_dir_all`, called from the delete job in `src-tauri/src/jobs/file_ops.rs`.

**Possible fix.** Prefer battle-tested OSS: the
[`trash`](https://crates.io/crates/trash) crate covers freedesktop trash,
the macOS Finder trash and the Windows Recycle Bin. Options:
- a "Move to trash" default with Shift+Delete for permanent delete, as most
  file managers do, behind a setting;
- trash only for local sources, so archives, remote, git and global selections
  keep their current semantics;
- Journal entries for trashed items, so they can be found later.

This touches `vfs.md` (a new driver capability) and `proto.md` (delete
options), so update both.

**Guide.** `files/operations.mdx` has a "Delete is permanent" caution. Remove
or reword it when trash lands.

## 2. Escape never reaches programs in the embedded terminal

**Observed.** In the embedded terminal (and therefore in Quick Edit),
Escape always returns focus to the listing, or un-maximizes the terminal. The
key is never sent to the program. This makes Quick Edit's **Vim** option
awkward, and affects any TUI that uses Escape (less, htop, mc, fzf…).
Ctrl+[ still sends ESC, which the guide recommends as a workaround.

**Where.** `src/components/PanelTerminal.tsx:154`, the
`attachCustomKeyEventHandler` that swallows every `Escape` keydown.

**Possible fix.** Some options, to be weighed against
`spex/keyboard.md` input ownership:
- a different "leave terminal" chord (for example Ctrl/Cmd+Escape or a
  double Escape within a short interval), passing a single Escape through;
- pass Escape through while the terminal is in the alternate screen buffer
  (full-screen TUIs like vim, less, htop), and keep it as "leave" at the shell
  prompt;
- a setting.

**Guide.** `viewing/editor.mdx` ("Escape in Vim" tip) and
`viewing/terminal.mdx` describe the current behaviour.

## 3. `thumbnails.md` says grid view is Shift+G

**Observed.** `spex/thumbnails.md:3` and `:67` describe the grid toggle as
Shift+G. The registry binds `panel.toggle_view_mode` to plain `G`; Shift+G is
`panel.git.actions` (Git Repository…).

**Fix.** Update the spec to say `G`, or better, name the command
(`panel.toggle_view_mode`) instead of a key so it can't drift again.

## 4. Two registry commands are titled "Open Entry"

**Observed.** `panel.open` (Enter) and `panel.open_entry` (Right, list view)
are both titled "Open Entry" in `src-tauri/src/ws/commands.rs:443` and `:450`.
They show up as two identical rows in the command palette, in
Settings → Shortcuts and in the site's generated keyboard reference.

**Possible fix.** Retitle one (for example "Open Entry (Right Arrow)", or
"Drill Into Entry"), then regenerate the site data:
`FBRO_UPDATE_SITE_DATA=1 cargo test --manifest-path src-tauri/Cargo.toml site_command_data_is_current`.

## 5. AGENTS.md says remote fbro uses system ssh

**Observed.** `AGENTS.md:210` ("Prefer battle-tested OSS…") gives remote fbro as
the example of choosing "SFTP over system ssh rather than a custom RPC". The
implementation embeds both ends: `russh` + `russh-sftp`, with no system `sshd`,
`ssh` or `sftp` (`spex/remote-fbro.md` §1, `src-tauri/src/remote/server.rs`).

**Fix.** Reword the example, for example "remote fbro became SFTP over an
embedded SSH stack (russh) rather than a custom RPC".

## 6. `bro.md` §5.2 lists an outdated set of bros

**Observed.** `spex/bro.md` §5.2 "v0 built-in discovery" lists mpv, VLC, Fedup,
native `7z` and native `tar`, and says (lines 446 and 453) that `7z` "is an
external native dependency, not a bundled tool". The code registers more bros
in `builtin_providers()` (`src-tauri/src/bro/providers/mod.rs:41`): Copy Paths,
Check hashes, Generate hashes and Kill (SIGKILL), built-in **zip** and **7z**
encoders ("Built-in 7z encoder"), and an extract provider (**unzip**, **un7z**
built in; **untar** via system `tar`). §7 does document the hash bros, so only
§5.2 is stale.

**Fix.** Update §5.2 to the current provider list and to say which archive
bros are built in and which use an external program.

