import { useCallback, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { FileViewer, type FileViewerProps } from "./FileViewer";
import { IMAGE_LIMIT } from "../viewers/ImageViewer";
import type { WsClient } from "../ws/client";

type ViewerAppProps = Omit<FileViewerProps, "fsPort" | "onClose"> & { windowLabel: string };

/** Native-window host. Loading and rendering live in FileViewer. */
export function ViewerApp({ windowLabel, ...props }: ViewerAppProps) {
  useEffect(() => {
    // WebKit can leave DOM focus on body after navigating a native window.
    // This host is the only input surface in that window; route those events
    // into its content root while controls keep their own native handling.
    const routeWindowKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || (event.target !== window && event.target !== document.body
        && event.target !== document.documentElement)) return;
      const content = document.querySelector<HTMLElement>("[data-viewer-content]");
      if (!content) return;
      content.focus({ preventScroll: true });
      const forwarded = new KeyboardEvent("keydown", {
        key: event.key, code: event.code, ctrlKey: event.ctrlKey, metaKey: event.metaKey,
        altKey: event.altKey, shiftKey: event.shiftKey, repeat: event.repeat,
        bubbles: true, cancelable: true,
      });
      content.dispatchEvent(forwarded);
      if (forwarded.defaultPrevented) event.preventDefault();
    };
    window.addEventListener("keydown", routeWindowKey);
    return () => window.removeEventListener("keydown", routeWindowKey);
  }, []);
  const params = new URLSearchParams(window.location.search);
  const rawCap = Number(params.get("imgcap"));
  const imageLimit = Number.isFinite(rawCap) && rawCap > 0 ? rawCap : IMAGE_LIMIT;
  const onTitleChange = useCallback((filename: string) => {
    const title = `${filename} — fbro viewer`;
    document.title = title;
    try { void getCurrentWindow().setTitle(title).catch(() => {}); } catch { /* Browser host. */ }
  }, []);
  const onMaximize = useCallback(() => {
    try { void getCurrentWindow().toggleMaximize().catch(() => {}); } catch { /* Browser host. */ }
  }, []);
  const onClose = useCallback((client: WsClient | null) => {
    const close = () => client?.sendIntent("CloseViewerWindow", undefined, { label: windowLabel });
    // macOS must leave native fullscreen before closing the window.
    try {
      const appWindow = getCurrentWindow();
      void appWindow.isFullscreen()
        .then(fullscreen => fullscreen ? appWindow.setFullscreen(false) : close())
        .catch(close);
    } catch { close(); }
  }, [windowLabel]);
  return <FileViewer {...props} imageLimit={imageLimit}
    fsPort={Number(params.get("fsport") ?? "0")}
    onTitleChange={onTitleChange} onMaximize={onMaximize} onClose={onClose} />;
}
