import { useRef, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingInfo } from "../store";
import type { WsClient } from "../ws/client";
import { IconCopy } from "@tabler/icons-react";
import { ResizableModal } from "./ResizableModal";

interface InfoModalProps {
  info: PendingInfo;
  client: WsClient;
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
  return (
    <button
      className="modal__job-param-copy"
      onClick={() => navigator.clipboard.writeText(text)}
      title="Copy to clipboard"
    ><IconCopy size={14} /></button>
  );
}

export function InfoModal({ info, client }: InfoModalProps) {
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
      return;
    }
    if (e.key === "p" || e.key === "P") {
      e.preventDefault();
      navigator.clipboard.writeText(info.fullPath);
      dispatch({ type: "SHOW_TRANSIENT_HINT", message: "Copied to clipboard" });
      return;
    }
  };

  return (
    <div className="modal-overlay" ref={overlayRef} tabIndex={-1} onClick={close} onKeyDown={handleKeyDown}>
      <ResizableModal
        modalId="info"
        client={client}
        ariaLabel="Info"
        defaultSize={{ width: 560, height: 420 }}
        minSize={{ width: 420, height: 280 }}
      >
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
                <td className="modal__info-label">Full <span className="modal__hotkey">p</span>ath</td>
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
                <>
                  <tr>
                    <td className="modal__info-label">Size</td>
                    <td className="modal__info-value">{formatSize(info.size)}</td>
                    <td />
                  </tr>
                  {info.size !== undefined && (
                    <tr>
                      <td className="modal__info-label">Size (bytes)</td>
                      <td className="modal__info-value">{info.size.toLocaleString()} B</td>
                      <td><CopyButton text={String(info.size)} /></td>
                    </tr>
                  )}
                </>
              )}
              <tr>
                <td className="modal__info-label">Modified</td>
                <td className="modal__info-value">{formatMtime(info.mtime)}</td>
                <td />
              </tr>
              {info.btime && (
                <tr>
                  <td className="modal__info-label">Created</td>
                  <td className="modal__info-value">{formatMtime(info.btime)}</td>
                  <td />
                </tr>
              )}
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
          <button className="modal__btn modal__btn--primary" onClick={close}>Close</button>
        </div>
      </ResizableModal>
    </div>
  );
}
