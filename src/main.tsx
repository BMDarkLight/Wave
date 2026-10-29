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
import MiniPlayer from "./components/MiniPlayer";
import { initTouchHoverGuards } from "./utils/touchHover";

// Defeat sticky :hover/:focus on Android WebView before first paint when possible.
initTouchHoverGuards();

// Wave draws its own menus; the system one (Reload, Inspect Element) never
// shows. React handles right-clicks on the app root first, so Wave's menus
// still open before this runs.
document.addEventListener("contextmenu", (event) => event.preventDefault());

// The mini player window loads this same page with `#mini`.
const isMiniPlayer = window.location.hash === "#mini";

// Render immediately - let the App component handle Tauri detection
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isMiniPlayer ? <MiniPlayer /> : <App />}
  </React.StrictMode>,
);
