import { useState, useRef, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingInfo } from "../store";

interface InfoModalProps {
  info: PendingInfo;
}

function formatSize(size: number | undefined): string {
  if (size === undefined) return "—";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  return `${(size / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function formatMtime(mtime: string | undefined): string {
  if (!mtime) return "—";
  const secs = parseInt(mtime, 10);
  if (isNaN(secs)) return "—";
  const d = new Date(secs * 1000);
  const p = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

function kindLabel(kind: string): string {
  switch (kind) {
    case "directory": return "Directory";
    case "file":      return "File";
    case "symlink":   return "Symbolic link";
    default:          return kind;
  }
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);

  const copy = () => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <button
      className="modal__info-copy"
      onClick={copy}
      title="Copy to clipboard"
    >
      {copied ? "Copied!" : "Copy"}
    </button>
  );
}

export function InfoModal({ info }: InfoModalProps) {
  const dispatch = useAppDispatch();
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    overlayRef.current?.focus();
  }, []);

  const close = () => dispatch({ type: "CLOSE_INFO" });

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Tab") e.preventDefault();
    if (e.key === "Escape" || e.key === "i" || e.key === "I" || e.key === "Enter") {
      e.preventDefault();
      close();
    }
  };

  return (
    <div className="modal-overlay" ref={overlayRef} tabIndex={-1} onClick={close} onKeyDown={handleKeyDown}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">Info</div>
        <div className="modal__body">
          <table className="modal__info-table">
            <tbody>
              <tr>
                <td className="modal__info-label">Name</td>
                <td className="modal__info-value">{info.name}</td>
                <td><CopyButton text={info.name} /></td>
              </tr>
              <tr>
                <td className="modal__info-label">Full path</td>
                <td className="modal__info-value" title={info.fullPath}>{info.fullPath}</td>
                <td><CopyButton text={info.fullPath} /></td>
              </tr>
              <tr>
                <td className="modal__info-label">Directory</td>
                <td className="modal__info-value" title={info.dirPath}>{info.dirPath}</td>
                <td><CopyButton text={info.dirPath} /></td>
              </tr>
              <tr>
                <td className="modal__info-label">Type</td>
                <td className="modal__info-value">{kindLabel(info.kind)}</td>
                <td />
              </tr>
              {info.kind !== "directory" && (
                <tr>
                  <td className="modal__info-label">Size</td>
                  <td className="modal__info-value">{formatSize(info.size)}</td>
                  <td />
                </tr>
              )}
              <tr>
                <td className="modal__info-label">Modified</td>
                <td className="modal__info-value">{formatMtime(info.mtime)}</td>
                <td />
              </tr>
              {info.permissions && (
                <tr>
                  <td className="modal__info-label">Permissions</td>
                  <td className="modal__info-value">{info.permissions}</td>
                  <td />
                </tr>
              )}
            </tbody>
          </table>
        </div>
        <div className="modal__actions">
          <button className="modal__btn" onClick={close}>Close</button>
        </div>
      </div>
    </div>
  );
}
