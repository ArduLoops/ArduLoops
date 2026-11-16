// Global CSS first: styles.css declares the cascade layers every other CSS file uses.
import "./styles.css";
import "./styles/buttons.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { DetachedRoot } from "./Detached";
import { announceMainWindow, detachedFromUrl, isDetachedWindow } from "./lib/detach";
import { applyDocumentLang } from "./i18n/i18n";
import { startNavMemory } from "./lib/nav";

applyDocumentLang();

function syncMobileClasses() {
  if (typeof navigator === "undefined" || typeof document === "undefined") return;
  const isAndroid = /Android/i.test(navigator.userAgent);
  const isMobile =
    isAndroid ||
    /iPhone|iPad|iPod/i.test(navigator.userAgent) ||
    // A detached panel's own window is narrow on a desktop; it is not a phone.
    (typeof window !== "undefined" && !isDetachedWindow() && window.matchMedia("(max-width: 768px)").matches);
  document.documentElement.classList.toggle("android", isAndroid);
  document.body.classList.toggle("android", isAndroid);
  document.documentElement.classList.toggle("mobile", isMobile);
  document.body.classList.toggle("mobile", isMobile);
}

syncMobileClasses();
if (typeof window !== "undefined") {
  window.addEventListener("resize", syncMobileClasses);
}

// A panel moved into its own window loads this same page with #detached=… and renders only that panel.
const detached = detachedFromUrl();
if (!detached) {
  announceMainWindow();
  // Before the first render: a bare URL opens on the last place (lib/nav.ts).
  startNavMemory();
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {detached ? <DetachedRoot initial={detached} /> : <App />}
  </StrictMode>,
);
