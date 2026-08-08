import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { initTouchHoverGuards } from "./utils/touchHover";

// Defeat sticky :hover/:focus on Android WebView before first paint when possible.
initTouchHoverGuards();

// Render immediately - let the App component handle Tauri detection
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
