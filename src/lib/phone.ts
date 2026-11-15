import { useSyncExternalStore } from "react";

/** The width where styles.css hides the studio and keeps only the side panel. */
const PHONE = "(max-width: 720px)";

function subscribe(onChange: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  const mq = window.matchMedia(PHONE);
  mq.addEventListener("change", onChange);
  return () => mq.removeEventListener("change", onChange);
}

function snapshot(): boolean {
  return typeof window !== "undefined" && window.matchMedia(PHONE).matches;
}

/** True on a phone-width screen: one pane at a time instead of the studio and the side panel. */
export function usePhone(): boolean {
  return useSyncExternalStore(subscribe, snapshot, () => false);
}
