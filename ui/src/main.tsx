import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { ConfirmHost } from "./components/Confirm";
import { Toaster } from "./components/Toast";
import "./styles.css";

// Toolbar leading-pad. The macOS title bar lives in its own system
// strip above our content (traffic lights up there, no overlay), so
// the toolbar just needs a normal gutter on every platform.
document.documentElement.style.setProperty("--toolbar-pad-left", "12px");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Toaster>
      <ConfirmHost>
        <App />
      </ConfirmHost>
    </Toaster>
  </React.StrictMode>,
);
