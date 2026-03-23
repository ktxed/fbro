import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { ViewerApp } from "./components/ViewerApp";
import "./styles.css";

const params = new URLSearchParams(window.location.search);

const root = ReactDOM.createRoot(document.getElementById("root")!);

if (params.get("viewer") === "1") {
  root.render(
    <React.StrictMode>
      <ViewerApp
        path={params.get("path") ?? ""}
        sourceId={params.get("source") ?? ""}
        filename={params.get("name") ?? params.get("path")?.split("/").pop() ?? ""}
      />
    </React.StrictMode>,
  );
} else {
  root.render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
