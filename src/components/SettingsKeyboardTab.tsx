import { useEffect, useRef, useState } from "react";
import { IconChevronDown, IconSearch, IconFilter } from "@tabler/icons-react";
import { useAppState } from "../store";
import type { WsClient } from "../ws/client";
import type { CommandBindingConflict, KeyBinding } from "../ws/types";
import { captureStroke, formatBinding, reviewBindingEdit } from "../commands/bindings";
import { ModalLayer } from "./ModalStack";
import { ModalDialog } from "./ModalDialog";
import { rovingIndexForKey } from "./modalKeyboard";

type Mutation =
  | { name: "SetCommandBindings"; commandId: string; bindings: KeyBinding[]; revision: number }
  | { name: "ResetCommandBindings"; commandId: string; revision: number }
  | { name: "ResetAllCommandBindings"; revision: number };
type Editor = { commandId: string; index: number | null; binding: KeyBinding | null; recording: boolean; revision: number; baseBindings: KeyBinding[]; stale: boolean };
const contextLabel = (binding: KeyBinding) => !binding.modes?.length || binding.modes.length === 2 ? "Both views" : binding.modes[0] === "list" ? "List only" : "Grid only";

export function SettingsKeyboardTab({ client }: { client: WsClient }) {
  const { commands, commandRevision, connected } = useAppState();
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState("all");
  const [selected, setSelected] = useState("");
  const [editor, setEditor] = useState<Editor | null>(null);
  const [resetAll, setResetAll] = useState(false);
  const [error, setError] = useState("");
  const [conflict, setConflict] = useState<{ mutation: Mutation; conflicts: CommandBindingConflict[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = useRef<{ requestId: string; mutation: Mutation } | null>(null);
  const recordRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const query = search.toLowerCase();
  const visible = Object.values(commands).filter(command =>
    (filter !== "customized" || command.customized) && (filter !== "unassigned" || command.bindings.length === 0)
    && `${command.title} ${command.category} ${command.id} ${command.bindings.map(binding => `${formatBinding(binding)} ${contextLabel(binding)}`).join(" ")}`.toLowerCase().includes(query));
  const selectedCommand = commands[selected] && visible.some(command => command.id === selected) ? commands[selected] : visible[0];

  useEffect(() => client.subscribeMessages(message => {
    if (!["CommandBindingsOk", "CommandBindingsConflict", "CommandBindingsFailed"].includes(message.type)
      || !("requestId" in message) || message.requestId !== pending.current?.requestId) return;
    const mutation = pending.current.mutation;
    pending.current = null;
    setBusy(false);
    if (message.type === "CommandBindingsOk") {
      setEditor(null); setResetAll(false); setConflict(null); setError("");
    } else if (message.type === "CommandBindingsConflict") {
      setConflict({ mutation: { ...mutation, revision: message.revision }, conflicts: message.conflicts });
    } else if (message.type === "CommandBindingsFailed") {
      setConflict(null);
      setError(message.message);
      if (message.code === "STALE_REVISION") setEditor(current => current ? { ...current, stale: true } : current);
    }
  }), [client]);
  useEffect(() => {
    if (!connected && pending.current) {
      pending.current = null; setBusy(false); setConflict(null); setError("Connection lost. Reconnect and review the current bindings before saving.");
    }
  }, [connected]);

  const send = (mutation: Mutation, override = false) => {
    if (pending.current || !connected) return;
    const requestId = crypto.randomUUID();
    pending.current = { requestId, mutation };
    setBusy(true); setError("");
    switch (mutation.name) {
      case "SetCommandBindings":
        client.sendCommandBindings({ name: mutation.name, args: { requestId, revision: mutation.revision, commandId: mutation.commandId, bindings: mutation.bindings, override } });
        break;
      case "ResetCommandBindings":
        client.sendCommandBindings({ name: mutation.name, args: { requestId, revision: mutation.revision, commandId: mutation.commandId, override } });
        break;
      case "ResetAllCommandBindings":
        client.sendCommandBindings({ name: mutation.name, args: { requestId, revision: mutation.revision } });
        break;
    }
  };
  const edit = (commandId: string, index: number | null) => {
    setError("");
    setEditor({ commandId, index, binding: index === null ? null : commands[commandId].bindings[index], recording: true, revision: commandRevision, baseBindings: commands[commandId].bindings, stale: false });
  };
  const cancelEditor = () => { if (!busy) { setEditor(null); setError(""); } };
  const save = () => {
    if (!editor?.binding || editor.recording || editor.stale || busy) return;
    const bindings = [...editor.baseBindings];
    if (editor.index === null) bindings.push(editor.binding);
    else bindings[editor.index] = editor.binding;
    send({ name: "SetCommandBindings", commandId: editor.commandId, bindings, revision: editor.revision });
  };

  return <div className="keyboard-settings" data-testid="keyboard-settings">
    <div className="keyboard-settings__search-row">
      <div className="keyboard-settings__search">
        <IconSearch size={16} aria-hidden="true" />
        <input ref={searchRef} aria-label="Search commands" data-testid="keyboard-search" value={search} onChange={event => setSearch(event.target.value)} placeholder="Name, command ID, or shortcut" />
      </div>
      <div className="keyboard-settings__filter">
        <IconFilter className="keyboard-settings__filter-icon" size={16} aria-hidden="true" />
        <select aria-label="Show commands" value={filter} data-testid="keyboard-filter" onChange={event => setFilter(event.target.value)}>
          <option value="all">All</option><option value="customized">Customized</option><option value="unassigned">Unassigned</option>
        </select>
        <IconChevronDown size={14} aria-hidden="true" />
      </div>
    </div>
    <p className="keyboard-settings__help">Record a single key combination, then Save to apply it. Application zoom (Ctrl/Cmd +, −, =, 0) and Shift+F10 / ContextMenu are reserved. Dialogs, text editors, terminals, and viewers keep their local keys.</p>
    <div className="keyboard-settings__commands" role="listbox" aria-label="Commands">
      {visible.map((command, index) => <button key={command.id} type="button" role="option" aria-selected={selectedCommand?.id === command.id}
        className="keyboard-settings__command" data-command-id={command.id} tabIndex={selectedCommand?.id === command.id ? 0 : -1}
        onFocus={() => setSelected(command.id)} onClick={() => setSelected(command.id)}
        onKeyDown={event => {
          const next = rovingIndexForKey(event.key, index, visible.length, "vertical");
          if (next === null) return;
          event.preventDefault();
          const node = event.currentTarget.parentElement?.children[next] as HTMLElement | undefined;
          node?.focus(); node?.scrollIntoView({ block: "nearest" });
        }}>
        <span><strong>{command.title}</strong><small>{command.category} · {command.id} · {command.customized ? "Customized" : "Default"}</small></span>
        <span>{command.bindings.length ? command.bindings.map((binding, i) => <span className="keyboard-settings__chip" key={i}><kbd>{formatBinding(binding)}</kbd> <small>{contextLabel(binding)}</small></span>) : <small>Unassigned</small>}</span>
      </button>)}
      {!visible.length && <p>No matching commands.</p>}
    </div>
    {selectedCommand && <div className="keyboard-settings__details" data-testid="keyboard-command-details">
      <strong>{selectedCommand.title}</strong>
      {selectedCommand.bindings.map((binding, index) => <div className="keyboard-settings__binding" key={index}>
        <kbd>{formatBinding(binding)}</kbd><span>{contextLabel(binding)}</span>
        <button className="modal__btn" data-testid={`keyboard-edit-${index}`} onClick={() => edit(selectedCommand.id, index)}>Edit</button>
        <button className="modal__btn" disabled={busy || !connected} data-testid={`keyboard-remove-${index}`} onClick={() => { searchRef.current?.focus(); send({ name: "SetCommandBindings", commandId: selectedCommand.id, bindings: selectedCommand.bindings.filter((_, i) => i !== index), revision: commandRevision }); }}>Remove</button>
      </div>)}
      <div className="keyboard-settings__actions">
        <button className="modal__btn" data-testid="keyboard-add" disabled={busy || !connected} onClick={() => edit(selectedCommand.id, null)}>Add shortcut</button>
        <button className="modal__btn" data-testid="keyboard-remove-all" disabled={busy || !connected || !selectedCommand.bindings.length} onClick={() => { searchRef.current?.focus(); send({ name: "SetCommandBindings", commandId: selectedCommand.id, bindings: [], revision: commandRevision }); }}>Remove all shortcuts</button>
        <button className="modal__btn" data-testid="keyboard-reset" disabled={!connected || !selectedCommand.customized} aria-disabled={busy || undefined} onClick={() => send({ name: "ResetCommandBindings", commandId: selectedCommand.id, revision: commandRevision })}>Reset to defaults</button>
      </div>
    </div>}
    {!editor && !resetAll && error && <p role="alert">{error}</p>}
    <button className="modal__btn" data-testid="keyboard-reset-all" disabled={busy || !connected} onClick={() => { setError(""); setResetAll(true); }}>Reset all to defaults</button>
    {editor && <ModalLayer modalId="keyboard-recorder" data-modal-owns-tab={editor.recording || undefined} initialFocusRef={recordRef} onKeyDownCapture={event => {
      if (!editor.recording) return;
      event.preventDefault(); event.stopPropagation();
      if (event.repeat) return;
      const stroke = captureStroke(event.nativeEvent);
      if (stroke) setEditor({ ...editor, recording: false, binding: { strokes: [stroke], modes: editor.binding?.modes ?? [] } });
    }} onKeyDown={event => {
      if (event.defaultPrevented) return;
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); cancelEditor(); }
    }}>
      <ModalDialog className="keyboard-settings__dialog shortcut-editor" ariaLabel={editor.index === null ? "Add shortcut" : "Edit shortcut"} testId="keyboard-recorder">
        <div className="modal__header">{editor.index === null ? "Add shortcut" : "Edit shortcut"}</div>
        <div className="modal__body shortcut-editor__body">
          <p className="shortcut-editor__command">{commands[editor.commandId]?.title}</p>
          <div ref={recordRef} tabIndex={-1} role="group" aria-label="Shortcut capture" aria-describedby="shortcut-editor-help"
            data-modal-owns-tab={editor.recording || undefined} data-recording={editor.recording}
            className="shortcut-editor__capture" data-testid="keyboard-capture" aria-live="polite">
            <span className="shortcut-editor__status">
              {editor.recording && <span className="shortcut-editor__dot" aria-hidden="true" />}
              {editor.recording ? "Recording" : "Shortcut"}
            </span>
            {editor.recording ? <span>Press a key combination…</span> : <kbd>{editor.binding && formatBinding(editor.binding)}</kbd>}
          </div>
          <p className="shortcut-editor__help" id="shortcut-editor-help">
            {editor.recording ? "Tab, Enter, and Escape can be recorded too." : "Save to apply this shortcut. Escape cancels."}
          </p>
          <div className="shortcut-editor__controls">
            <label className="shortcut-editor__context">
              <span>View context</span>
              <span className="shortcut-editor__select">
                <select data-testid="keyboard-context" disabled={editor.recording || busy} value={editor.binding?.modes?.length === 1 ? editor.binding.modes[0] : "both"} onChange={event => editor.binding && setEditor({ ...editor, binding: { ...editor.binding, modes: event.target.value === "both" ? [] : [event.target.value as "list" | "grid"] } })}>
                  <option value="both">Both views</option><option value="list">List only</option><option value="grid">Grid only</option>
                </select>
                <IconChevronDown size={14} aria-hidden="true" />
              </span>
            </label>
            <button className="modal__btn" data-testid="keyboard-record-again" disabled={editor.recording || busy} onClick={() => { setEditor({ ...editor, recording: true }); recordRef.current?.focus(); }}>Record again</button>
          </div>
          {error && <p className="shortcut-editor__error" role="alert" data-testid="keyboard-error">{error}</p>}
          {editor.stale && <div className="shortcut-editor__review">
            <p>Current shortcuts: {commands[editor.commandId].bindings.map(binding => `${formatBinding(binding)} (${contextLabel(binding)})`).join("; ") || "Unassigned"}</p>
            <button className="modal__btn" data-testid="keyboard-review-current" onClick={() => {
              const reviewed = reviewBindingEdit(editor.baseBindings, editor.index, commands[editor.commandId].bindings);
              if (!reviewed) { setError("The original binding changed or was removed. Cancel and reopen the editor to choose the current binding."); return; }
              setEditor({ ...editor, index: reviewed.index, baseBindings: reviewed.bindings, revision: commandRevision, stale: false });
              setError("");
            }}>Use reviewed current bindings</button>
          </div>}
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" data-testid="keyboard-save" disabled={!editor.binding || editor.recording || editor.stale || !connected} aria-disabled={busy || undefined} onClick={save}>{busy ? "Saving…" : "Save"}</button>
          <button className="modal__btn" data-testid="keyboard-cancel" disabled={busy} onClick={cancelEditor}>Cancel</button>
        </div>
      </ModalDialog>
    </ModalLayer>}
    {resetAll && <ModalLayer modalId="keyboard-reset-all" onKeyDown={event => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (!busy) setResetAll(false); }
    }}><ModalDialog className="keyboard-settings__dialog" ariaLabel="Reset all keyboard shortcuts" role="alertdialog">
      <div className="modal__header">Reset all keyboard shortcuts?</div>
      <div className="modal__body"><p>This clears every customization and restores the platform defaults.</p>{error && <p role="alert">{error}</p>}</div>
      <div className="modal__actions">
        <button className="modal__btn modal__btn--primary" data-testid="keyboard-confirm-reset-all" disabled={busy || !connected} onClick={() => send({ name: "ResetAllCommandBindings", revision: commandRevision })}>{busy ? "Resetting…" : "Reset all"}</button>
        <button className="modal__btn" data-modal-initial-focus data-testid="keyboard-cancel-reset-all" disabled={busy} onClick={() => setResetAll(false)}>Cancel</button>
      </div>
    </ModalDialog></ModalLayer>}
    {conflict && <ModalLayer modalId="keyboard-conflict" onKeyDown={event => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (!busy) setConflict(null); }
    }}><ModalDialog className="keyboard-settings__dialog shortcut-conflicts" ariaLabel="Shortcut conflicts" role="alertdialog">
      <div className="modal__header">Shortcut conflicts</div>
      <div className="modal__body shortcut-conflicts__body">
        <p>Already assigned to:</p>
        <ul className="shortcut-conflicts__commands">
          {conflict.conflicts.map((item, index) => <li className="shortcut-conflicts__command" key={`${item.commandId}-${index}`}>
            <strong data-testid="keyboard-conflict-command">{item.title}</strong>
            <span className="shortcut-conflicts__contexts" aria-label="Affected views">
              {item.modes.map(mode => <span className="shortcut-conflicts__context" data-testid="keyboard-conflict-context" key={mode}>{mode === "list" ? "List view" : "Grid view"}</span>)}
            </span>
          </li>)}
        </ul>
        <p className="shortcut-conflicts__help">Override reassigns the shortcut in these views. Other shortcuts stay assigned.</p>
      </div>
      <div className="modal__actions">
        <button className="modal__btn modal__btn--primary" data-testid="keyboard-override" disabled={busy || !connected} onClick={() => send(conflict.mutation, true)}>{busy ? "Saving…" : "Override"}</button>
        <button className="modal__btn" data-testid="keyboard-abort" data-modal-initial-focus disabled={busy} onClick={() => setConflict(null)}>Abort</button>
      </div>
    </ModalDialog></ModalLayer>}
  </div>;
}
