import { useRef, useState, type ReactNode } from "react";
import { useAppDispatch, useAppState } from "../store";
import type { PendingInfo } from "../store";
import type { WsClient } from "../ws/client";
import { IconArrowUpRight, IconChevronRight } from "@tabler/icons-react";
import { ResizableModal } from "./ResizableModal";
import { ModalLayer } from "./ModalStack";
import { useCopyToClipboard } from "../hooks/useCopyToClipboard";
import { CopyButton } from "./CopyButton";
import { isLetterKey } from "./modalKeyboard";

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

/** Same fallbacks as the listing panel when no MIME icon is available. */
function kindFallbackIcon(kind: string): ReactNode {
  switch (kind) {
    case "directory": return <IconChevronRight size={14} />;
    case "symlink":   return <IconArrowUpRight size={14} />;
    default:          return null;
  }
}

/** Mirrors Panel EntryIcon: BE icon via fileserver, else kind fallback. */
function TypeIcon({ kind, iconName }: { kind: string; iconName?: string }) {
  const [failed, setFailed] = useState(false);
  const { fileServerPort } = useAppState();
  const icon = iconName && !failed && fileServerPort > 0 ? (
    <img
      src={`http://127.0.0.1:${fileServerPort}/__icon__/${iconName}`}
      alt=""
      draggable={false}
      onError={() => setFailed(true)}
    />
  ) : (
    kindFallbackIcon(kind)
  );
  if (!icon) return null;
  return <span className="modal__info-type-icon" aria-hidden>{icon}</span>;
}

export function InfoModal({ info, client }: InfoModalProps) {
  const dispatch = useAppDispatch();
  const copyToClipboard = useCopyToClipboard();
  const closeRef = useRef<HTMLButtonElement>(null);

  const close = () => dispatch({ type: "CLOSE_INFO" });

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape" || isLetterKey(e, "i")) {
      e.preventDefault();
      close();
      return;
    }
    if (isLetterKey(e, "p")) {
      e.preventDefault();
      copyToClipboard(info.fullPath, "full path");
      return;
    }
  };

  return (
    <ModalLayer modalId="info" initialFocusRef={closeRef} onClick={close} onKeyDown={handleKeyDown}>
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
            <tbody className="modal__info-group">
              <tr>
                <td className="modal__info-label">Name</td>
                <td className="modal__info-value">{info.name}</td>
                <td><CopyButton text={info.name} what="name" /></td>
              </tr>
              <tr>
                <td className="modal__info-label">Type</td>
                <td className="modal__info-value modal__info-type">
                  <TypeIcon kind={info.kind} iconName={info.iconName} />
                  {kindLabel(info.kind)}
                </td>
                <td />
              </tr>
            </tbody>
            <tbody className="modal__info-group">
              <tr>
                <td className="modal__info-label">Full <span className="modal__hotkey">p</span>ath</td>
                <td className="modal__info-value" title={info.fullPath}>{info.fullPath}</td>
                <td><CopyButton text={info.fullPath} what="full path" /></td>
              </tr>
              <tr>
                <td className="modal__info-label">Directory</td>
                <td className="modal__info-value" title={info.dirPath}>{info.dirPath}</td>
                <td><CopyButton text={info.dirPath} what="directory path" /></td>
              </tr>
            </tbody>
            <tbody className="modal__info-group">
              {info.kind !== "directory" && (
                <tr>
                  <td className="modal__info-label">Size</td>
                  <td className="modal__info-value">
                    {formatSize(info.size)}
                    {info.size !== undefined && info.size >= 1024 && (
                      <> ({info.size.toLocaleString()} B)</>
                    )}
                  </td>
                  <td>
                    {info.size !== undefined && <CopyButton text={String(info.size)} what="size" />}
                  </td>
                </tr>
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
          <button ref={closeRef} className="modal__btn modal__btn--primary" onClick={close}>Close</button>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
