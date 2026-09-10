import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { DiffViewerApp } from "./components/DiffViewerApp";
import { ViewerApp } from "./components/ViewerApp";
import "./styles.css";
import throbberUrl from "./assets/win98-flag-throbber-64nn.gif";

const params = new URLSearchParams(window.location.search);

const root = ReactDOM.createRoot(document.getElementById("root")!);

if (params.get("diff") === "1") {
  root.render(
    <React.StrictMode>
      <DiffViewerApp
        requestId={params.get("request") ?? ""}
        windowLabel={params.get("label") ?? ""}
        initialColorScheme={params.get("colorScheme")}
      />
    </React.StrictMode>,
  );
} else if (params.get("viewer") === "1") {
  const sizeParam = params.get("size");
  root.render(
    <React.StrictMode>
      <ViewerApp
        path={params.get("path") ?? ""}
        sourceId={params.get("source") ?? ""}
        filename={params.get("name") ?? params.get("path")?.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? ""}
        windowLabel={params.get("label") ?? ""}
        panelId={params.get("panel") ?? ""}
        isLocal={params.get("local") === "1"}
        nativeFileSize={sizeParam !== null ? Number(sizeParam) : null}
        initialColorScheme={params.get("colorScheme")}
        initialFindQuery={params.get("find")}
      />
    </React.StrictMode>,
  );
} else {
  // Only the main window renders panels. Warm the browser cache before a panel
  // can enter its loading state without touching viewer/compare webviews.
  const throbberPreload = document.createElement("link");
  throbberPreload.rel = "preload";
  throbberPreload.as = "image";
  throbberPreload.href = throbberUrl;
  document.head.appendChild(throbberPreload);

  root.render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
