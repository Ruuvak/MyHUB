import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { windowLabel } from "./lib/ipc";
import { applyTheme } from "./lib/theme";
import { SettingsProvider } from "./state/settings";
import { FlyoutView } from "./views/FlyoutView";
import "./styles/index.css";

// Resolve the theme from the OS before first paint; the settings load then takes over.
applyTheme("system");

// The same frontend serves two windows. The flyout is a compact card on a transparent window.
const isFlyout = windowLabel() === "flyout";
if (isFlyout) document.documentElement.dataset.view = "flyout";

const root = document.getElementById("root");
if (!root) {
  throw new Error("Missing #root element");
}

createRoot(root).render(
  <StrictMode>
    <SettingsProvider>{isFlyout ? <FlyoutView /> : <App />}</SettingsProvider>
  </StrictMode>,
);
