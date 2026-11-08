import { useEffect, useState } from "react";
import { APP_HTTP } from "../mav/link";
import { usePicked } from "../mav/view";
import { versionKnown } from "./vehicleHeader";

/** Fired after the vehicle's note or profile is saved, so every view of it reloads. */
export const VEHICLE_SAVED = "arduloops:vehicle-saved";

/**
 * The saved record of the linked board (its note and profile), reloaded when the board changes or is saved.
 * Null with no link or before it loads.
 */
export function useLinkedController<T>(): T | null {
  const identity = usePicked((s) => `${s.ok ? 1 : 0}|${s.boot_uid ?? ""}|${s.frame}|${versionKnown(s) ? 1 : 0}`);
  const [controller, setController] = useState<T | null>(null);
  useEffect(() => {
    let ac = new AbortController();
    const load = () => {
      ac.abort();
      ac = new AbortController();
      void fetch(`${APP_HTTP}/firmware-library`, { cache: "no-store", signal: ac.signal })
        .then((response) => (response.ok ? response.json() : null))
        .then((value: { controller?: T | null } | null) => setController(value?.controller ?? null))
        .catch(() => {});
    };
    load();
    window.addEventListener(VEHICLE_SAVED, load);
    return () => {
      ac.abort();
      window.removeEventListener(VEHICLE_SAVED, load);
    };
  }, [identity]);
  return controller;
}
