import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { bootstrapI18n } from "./i18n";
import "./index.css";

// Wait for the Rust-side preference (if any) before painting so the first
// frame already shows the persisted language. Falls back to the detector
// result immediately when the invoke rejects.
void bootstrapI18n().finally(() => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
});
