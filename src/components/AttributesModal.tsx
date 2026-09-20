import { useState, useRef, useEffect, useCallback } from "react";
import { useAppDispatch } from "../store";
import type { PendingAttributes } from "../store";
import type { WsClient } from "../ws/client";
import { ModalLayer } from "./ModalStack";
import { isNativeActivationTarget } from "./modalKeyboard";

interface AttributesModalProps {
  attributes: PendingAttributes;
  client: WsClient;
}

function parseMode(modeStr: string): number {
  let mode = 0;
  for (let i = 0; i < 9; i++) {
    const bit = 8 - i;
    const ch = modeStr[i] ?? "-";
    if ((i % 3 === 0 && ch === "r") || (i % 3 === 1 && ch === "w") || (i % 3 === 2 && ch === "x")) {
      mode |= (1 << bit);
    }
  }
  return mode;
}

function formatMode(mode: number): string {
  const bits = (i: number) => [
    mode & (1 << (8 - i * 3)) ? "r" : "-",
    mode & (1 << (7 - i * 3)) ? "w" : "-",
    mode & (1 << (6 - i * 3)) ? "x" : "-",
  ].join("");
  return `${bits(0)}${bits(1)}${bits(2)}`;
}

function modeToOct(mode: number): string {
  return ((mode >> 6) & 7).toString() + ((mode >> 3) & 7).toString() + (mode & 7).toString();
}

function parseOctMode(oct: string): number | null {
  if (!/^[0-7]{1,3}$/.test(oct)) return null;
  return parseInt(oct, 8);
}

const ROWS = [
  { label: "Owner", shift: 6 },
  { label: "Group", shift: 3 },
  { label: "Others", shift: 0 },
];

export function AttributesModal({ attributes, client }: AttributesModalProps) {
  const dispatch = useAppDispatch();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const overlayRef = useRef<HTMLDivElement>(null);
  const initialMode = parseMode(attributes.currentMode);
  const [mode, setMode] = useState(initialMode);
  const [octInput, setOctInput] = useState(modeToOct(initialMode));

  useEffect(() => {
    overlayRef.current?.focus();
  }, []);

  useEffect(() => {
    if (attributes.error) {
      setSubmitting(false);
      setError(attributes.error);
    }
  }, [attributes.error]);

  const close = useCallback(() => dispatch({ type: "CLOSE_ATTRIBUTES" }), [dispatch]);

  const updateMode = useCallback((newMode: number) => {
    setMode(newMode);
    setOctInput(modeToOct(newMode));
    setError(null);
  }, []);

  const handleCheckbox = useCallback(
    (_shift: number, bit: number, checked: boolean) => {
      const newMode = checked ? mode | (1 << bit) : mode & ~(1 << bit);
      updateMode(newMode);
    },
    [mode, updateMode],
  );

  const handleOctChange = useCallback(
    (value: string) => {
      setOctInput(value);
      const m = parseOctMode(value);
      if (m !== null) {
        setMode(m);
        setError(null);
      }
    },
    [],
  );

  const submit = useCallback(() => {
    if (submitting || parseOctMode(octInput) === null) return;
    setSubmitting(true);
    setError(null);
    client.sendIntent("SetPermissions", undefined, {
      panelId: attributes.panelId,
      paths: attributes.paths,
      mode,
    });
  }, [submitting, octInput, mode, client, attributes.panelId, attributes.paths]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.defaultPrevented) return;
      if (e.key === "Escape") { e.preventDefault(); close(); }
      if (e.key === "Enter" && !isNativeActivationTarget(e.target)) {
        e.preventDefault();
        submit();
      }
    },
    [submit, close],
  );

  const singleFile = attributes.names.length === 1;
  const symStr = formatMode(mode);
  const canSubmit = !submitting && parseOctMode(octInput) !== null;

  return (
    <ModalLayer modalId="attributes" ref={overlayRef} onClick={close} onKeyDown={handleKeyDown}>
      <div className="modal modal--fixed" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">Set Permissions</div>
        <div className="modal__body">
          <p className="modal__summary">
            {singleFile
              ? <>Change permissions for <strong>{attributes.names[0]}</strong></>
              : <>Change permissions for <strong>{attributes.names.length}</strong> items</>
            }
          </p>
          {!singleFile && (
            <ul className="modal__file-list">
              {attributes.names.slice(0, 20).map((name: string, i: number) => (
                <li key={i}>{name}</li>
              ))}
              {attributes.names.length > 20 && <li>…and {attributes.names.length - 20} more</li>}
            </ul>
          )}

          <div className="attrs-grid">
            <div className="attrs-grid__header">
              <span />
              <span className="attrs-grid__col">Read</span>
              <span className="attrs-grid__col">Write</span>
              <span className="attrs-grid__col">Execute</span>
            </div>
            {ROWS.map(({ label, shift }) => (
              <div key={label} className="attrs-grid__row">
                <span className="attrs-grid__label">{label}</span>
                {[2, 1, 0].map((offset) => {
                  const bit = shift + offset;
                  const checked = !!(mode & (1 << bit));
                  return (
                    <label key={offset} className="attrs-grid__checkbox">
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(e) => handleCheckbox(shift, bit, e.target.checked)}
                      />
                    </label>
                  );
                })}
              </div>
            ))}
          </div>

          <div className="attrs-octal">
            <label className="attrs-octal__label">Octal:</label>
            <input
              className="attrs-octal__input"
              value={octInput}
              onChange={(e) => handleOctChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key !== "Enter") return;
                e.preventDefault();
                e.stopPropagation();
                submit();
              }}
              spellCheck={false}
              autoComplete="off"
            />
            <span className="attrs-octal__preview">{symStr}</span>
          </div>

          {(error) && (
            <p className="modal__rename-error">{error}</p>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            disabled={!canSubmit}
            onClick={submit}
          >
            {submitting ? "Applying…" : "Apply"}
          </button>
          <button className="modal__btn" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </ModalLayer>
  );
}
