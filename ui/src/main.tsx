import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { ConfirmHost } from "./components/Confirm";
import { Toaster } from "./components/Toast";
import "./styles.css";

// Toolbar leading-pad. The macOS traffic lights live in a separate
// system-drawn title bar above our content, so we don't need to clear
// them inside the toolbar — every platform just wants a normal gutter.
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
