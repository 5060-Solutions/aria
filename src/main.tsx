import React from "react";
import ReactDOM from "react-dom/client";
// Self-hosted Figtree. Imported here so Vite fingerprints and bundles the
// woff2 files — the app no longer fetches fonts from Google at runtime.
import "./assets/fonts/fonts.css";
import { App } from "./App";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
