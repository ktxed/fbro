import { useCallback, useEffect, useImperativeHandle, useRef, useState, type KeyboardEvent, type MouseEvent, type Ref } from "react";
import { IconExternalLink, IconX } from "@tabler/icons-react";
import type { AppState } from "../store";
import type { ViewerTarget } from "../ws/types";
import type { WsClient } from "../ws/client";
import { FileViewer, type FileViewerHandle } from "./FileViewer";
import { isVideoExt } from "../viewers/media";
import { IMAGE_DIRECT_LIMIT } from "../viewers/DirectFileViewer";
import { findMatchingCommand, formatPrimaryBinding, getCommand, GLOBAL_SHORTCUT_COMMAND_IDS } from "../commands/interactive";
import { zoomAction } from "../commands/bindings";
import { useFocusReturn } from "../hooks/useFocusReturn";
import { isAvailableFocusTarget } from "../utils/focusReturn";

export interface EmbeddedViewerHandle {
  toggleVideoPlayback: (repeat: boolean) => boolean;
}

interface Props {
  ref?: Ref<EmbeddedViewerHandle>;
  panelId: string;
  sourcePanelId: string;
  state: AppState;
  client: WsClient;
  blocked: boolean;
  onClose: () => void;
}

/** Quick-preview host: target resolution and panel focus, with shared file content. */
export function EmbeddedViewerPanel({ ref, panelId, sourcePanelId, state, client, blocked, onClose }: Props) {
  const [target, setTarget] = useState<ViewerTarget | { kind: "loading" }>({ kind: "loading" });
  const rootRef = useRef<HTMLDivElement | null>(null);
  const viewerRef = useRef<FileViewerHandle | null>(null);
  const contentHandoffRef = useRef(false);
  const [contentRequestId, setContentRequestId] = useState("");
  const focusReturnRef = useFocusReturn(sourcePanelId);
  const requestRef = useRef("");
  const setRootRef = useCallback((element: HTMLDivElement | null) => {
    rootRef.current = element;
    return focusReturnRef(element);
  }, [focusReturnRef]);
  const sourcePanel = state.panels[sourcePanelId];
  const focused = state.focus === panelId;
  const { ludicrousMode, maxImageSizeMb, enableHeic2any, colorScheme } = state.settings.general;
  const imageLimit = ludicrousMode ? maxImageSizeMb * 1024 * 1024 : IMAGE_DIRECT_LIMIT;
  const file = target.kind === "file" ? target.file : null;
  useImperativeHandle(ref, () => ({
    toggleVideoPlayback: repeat => !blocked && !!file && isVideoExt(file.filename)
      && (viewerRef.current?.togglePlayback(repeat) ?? false),
  }), [blocked, file]);
  const fileKey = file ? JSON.stringify([file.sourceId, file.path, contentRequestId]) : "";
  const viewCommand = getCommand(state.commands, "file.view");
  const viewBinding = viewCommand ? formatPrimaryBinding(viewCommand) : "";

  useEffect(() => client.subscribeMessages(message => {
    if (message.type === "ViewerFileReady" && message.requestId === requestRef.current) {
      const root = rootRef.current;
      // Move focus to the persistent host before replacing a keyed content
      // session. A later effect hands it to the new content, never an outside owner.
      if (root?.querySelector("[data-viewer-content]")?.contains(document.activeElement)) {
        contentHandoffRef.current = true;
        root.focus({ preventScroll: true });
      }
      setTarget(message.target);
      setContentRequestId(message.requestId);
    }
  }), [client]);

  // Cursor/entry identity is only an invalidation signal. The backend resolves
  // the actual target, including search rows, symlinks and opaque VFS paths.
  const entry = sourcePanel?.entries[sourcePanel.cursor - (sourcePanel.windowStart ?? 0)];
  const revision = JSON.stringify([sourcePanelId, sourcePanel?.sourceId, sourcePanel?.path,
    sourcePanel?.cursor, entry?.name, entry?.sourcePath, entry?.size, entry?.mtime, imageLimit]);
  useEffect(() => {
    const requestId = crypto.randomUUID();
    requestRef.current = requestId;
    const timer = setTimeout(() => {
      client.sendIntent("ResolveViewerFile", undefined, { panelId: sourcePanelId, requestId });
    }, 80);
    return () => { clearTimeout(timer); requestRef.current = ""; };
  }, [client, sourcePanelId, revision]);

  useEffect(() => {
    const root = rootRef.current;
    if (!focused || blocked || !root || root.contains(document.activeElement)) return;
    (root.querySelector<HTMLElement>("[data-viewer-content]") ?? root).focus({ preventScroll: true });
  }, [focused, blocked]);

  useEffect(() => {
    contentHandoffRef.current = false;
    const root = rootRef.current;
    if (!blocked && root && document.activeElement === root) {
      root.querySelector<HTMLElement>("[data-viewer-content]")?.focus({ preventScroll: true });
    }
  }, [fileKey, blocked]);

  const openViewer = () => {
    if (!file || blocked) return;
    client.sendIntent("ViewFile", undefined, {
      panelId: sourcePanelId, sourceId: file.sourceId, filePath: file.path,
      filename: file.filename, initialFindQuery: file.initialFindQuery,
    });
  };
  const returnToListing = () => client.sendIntent("SwitchFocus", undefined, { to: sourcePanelId });

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented || blocked) return;
    if (findMatchingCommand(state.commands, ["file.view"], event.nativeEvent, state.panels[sourcePanelId]?.viewMode ?? "list")) {
      event.preventDefault(); event.stopPropagation(); openViewer(); return;
    }
    if (findMatchingCommand(state.commands, ["panel.viewer.embed"], event.nativeEvent, state.panels[sourcePanelId]?.viewMode ?? "list")) {
      event.preventDefault(); event.stopPropagation(); onClose(); return;
    }
    // Application commands/zoom remain global; file operations for the hidden
    // listing never receive quick-preview keys.
    if (zoomAction(event.nativeEvent) !== null
      || ((event.ctrlKey || event.metaKey || event.altKey)
        && findMatchingCommand(state.commands, GLOBAL_SHORTCUT_COMMAND_IDS.filter(id => id.startsWith("app.")), event.nativeEvent, state.panels[sourcePanelId]?.viewMode ?? "list"))) return;
    if (event.key === "Escape") {
      event.preventDefault(); event.stopPropagation(); onClose(); return;
    }
    if (event.key === "Tab") {
      const root = rootRef.current;
      if (!root) return;
      const controls = Array.from(root.querySelectorAll<HTMLElement>(
        "button:not(:disabled), [tabindex='0'], video[controls], audio[controls], a[href]",
      )).filter(isAvailableFocusTarget);
      const active = document.activeElement;
      // The content root is the initial Tab destination. Native Tab traverses
      // its controls and the header; the boundaries return to the source list.
      if ((event.shiftKey && (active === controls[0] || active === root))
        || (!event.shiftKey && active === controls[controls.length - 1])) {
        event.preventDefault(); returnToListing();
      }
      event.stopPropagation();
      return;
    }
    // A quick preview never forwards its keys to file-operation shortcuts for
    // the hidden listing in this slot. Content already handled its own keys.
    event.stopPropagation();
  };

  const handleMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (blocked) return;
    const element = event.target instanceof Element ? event.target : null;
    // Leave native controls and selectable content alone. Focusing the content
    // root enables media keys without cancelling the pointer's default action.
    if (!element?.closest("button, input, textarea, select, video, audio, embed, a")) {
      rootRef.current?.querySelector<HTMLElement>("[data-viewer-content]")?.focus({ preventScroll: true });
    }
  };

  return (
    <div className={`embedded-viewer${focused ? " embedded-viewer--focused panel--focused" : ""}`}
      data-testid={`preview-${panelId}`} data-panel-id={panelId} data-preview-request-id={contentRequestId} tabIndex={0}
      ref={setRootRef}
      onKeyDown={handleKeyDown} onMouseDown={handleMouseDown}
      onFocus={event => {
        if (event.target === event.currentTarget && !blocked && !contentHandoffRef.current) {
          rootRef.current?.querySelector<HTMLElement>("[data-viewer-content]")?.focus({ preventScroll: true });
        }
        if (!focused && !blocked) client.sendIntent("SwitchFocus", undefined, { to: panelId });
      }}>
      <div className="embedded-viewer__content">
        {file ? <FileViewer key={fileKey} ref={viewerRef}
          path={file.path} sourceId={file.sourceId} filename={file.filename}
          panelId={sourcePanelId} isLocal={file.isLocal} nativeFileSize={file.fileSize}
          fsPort={state.fileServerPort} imageLimit={imageLimit} enableHeic2any={enableHeic2any}
          initialColorScheme={colorScheme} presentation="quick" blocked={blocked} onClose={onClose} />
          : <div className="embedded-viewer__placeholder" role={target.kind === "error" ? "alert" : "status"}>
            {target.kind === "directory" ? "Directory" : target.kind === "empty" ? "No file selected"
              : target.kind === "error" ? target.message : "Loading…"}
          </div>}
      </div>
      <div className="embedded-viewer__header">
        <span className="embedded-viewer__header-label">
          {file?.filename ?? (target.kind === "directory" ? target.name : "Quick preview")}
        </span>
        <button type="button" className="viewer__mode-btn" data-testid="preview-open-viewer"
          disabled={!file || blocked} onClick={openViewer}
          title={`Open viewer${viewBinding ? ` (${viewBinding})` : ""}`}>
          <IconExternalLink size={14} aria-hidden /> Open viewer
          {viewBinding && <kbd>{viewBinding}</kbd>}
        </button>
        <button type="button" className="embedded-viewer__close" data-testid="preview-close"
          title="Close preview (Esc)" aria-label="Close preview" onClick={onClose}>
          <IconX size={14} aria-hidden />
        </button>
      </div>
    </div>
  );
}
