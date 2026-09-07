/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

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
