/**
 * Global application network and environment configuration.
 * Centralizes local bridge ports, cloud origins, and OAuth callback schemes.
 */

/** Local MAVLink HTTP bridge port. VITE_BRIDGE_PORT points a dev build at a second bridge (a second "device"). */
export const APP_HTTP_PORT = Number(
  (typeof import.meta !== "undefined" && import.meta.env?.VITE_BRIDGE_PORT) || 8767,
);

/** Local MAVLink HTTP bridge (browser and desktop) */
export const APP_HTTP = `http://127.0.0.1:${APP_HTTP_PORT}`;

/** Cloud & Auth hosting origin (Firebase Hosting or custom domain) */
export const CLOUD_AUTH_ORIGIN =
  (typeof import.meta !== "undefined" && import.meta.env?.VITE_AUTH_ORIGIN) ||
  "https://arduloops.web.app";

/** Cloud cabinet / account page */
export const CLOUD_ACCOUNT_URL = `${CLOUD_AUTH_ORIGIN}/account`;

/** Support packs, monthly support, and AI credits in the cabinet */
export const CLOUD_PLANS_URL = `${CLOUD_AUTH_ORIGIN}/me#plans`;

/**
 * The Android app is installed from Google Play, which does not allow a link to pay outside Play for
 * something used in the app (AI credits). It links to the cabinet overview only and names no support.
 */
export const IS_ANDROID = typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);

/** Mobile custom-scheme callback for OAuth */
export const AUTH_CALLBACK_MOBILE = "arduloops://auth";

/** Desktop HTTP callback for OAuth */
export const AUTH_CALLBACK_DESKTOP = `${APP_HTTP}/auth/callback`;

/** Platform-aware login URL generation with safe fallback */
export function defaultLoginUrl(): string {
  const isMobile =
    typeof navigator !== "undefined" &&
    /Android|iPhone|iPad|iPod/i.test(navigator.userAgent);
  const ret = isMobile ? AUTH_CALLBACK_MOBILE : AUTH_CALLBACK_DESKTOP;
  return `${CLOUD_AUTH_ORIGIN}/login?return=${encodeURIComponent(ret)}`;
}
