/**
 * Where the main window is, in the URL query: `?frame=copter&ws=controller&pane=setup&step=radio`, plus `?board=<key>`.
 *
 * No router: the screens read these params with `useNavParam` and keep their live state (link, 3D scene,
 * plot streams) mounted. A tab change is `pushState`, so Back and Forward move between tabs and a reload keeps
 * the place. The hash stays free for detached windows (lib/detach.ts).
 *
 * The levels nest: frame > ws > pane > step. Changing one level clears the levels below it unless the same call
 * gives them. A value a screen does not know (a plane-only step on a copter) shows that screen's default and
 * stays in the URL.
 *
 * App links: `arduloops://open?frame=copter&ws=controller&pane=logs`, or the same query as a relative link
 * (`?frame=copter&ws=controller&pane=logs`). The assistant offers them; clicking one moves the main window
 * here instead of opening a browser. The desktop app registers the scheme with the OS (src-tauri/src/lib.rs):
 * the link that started the app comes from `take_app_link`, and a link while it runs is an `app-link` event.
 */
import { useSyncExternalStore } from "react";
import { isDetachedWindow, isTauri, onWindowMsg, postWindowMsg } from "./detach";

export const NAV_LEVELS = ["frame", "ws", "pane", "step"] as const;
export type NavKey = (typeof NAV_LEVELS)[number] | "board";
/** A key set to null is removed from the URL. A key left out keeps its value (unless a level above it changes). */
export type Nav = Partial<Record<NavKey, string | null>>;

const NAV_KEYS: readonly NavKey[] = [...NAV_LEVELS, "board"];
const NAV_EVENT = "arduloops:nav";
export const APP_SCHEME = "arduloops:";

/** The URL names a workspace by its tab label where that differs from the id. */
const WS_URL: Record<string, string> = { firmware: "controller", scripts: "lua" };
const WS_ID: Record<string, string> = Object.fromEntries(Object.entries(WS_URL).map(([id, name]) => [name, id]));

/** The `ws` value for a workspace id. */
export function wsParam(id: string): string {
  return WS_URL[id] ?? id;
}

function search(): URLSearchParams {
  return new URLSearchParams(typeof window === "undefined" ? "" : window.location.search);
}

/** One param; `ws` comes back as the workspace id. */
export function readNavParam(key: NavKey): string | null {
  const v = search().get(key) || null;
  return key === "ws" && v ? WS_ID[v] ?? v : v;
}

function subscribe(fn: () => void): () => void {
  window.addEventListener("popstate", fn);
  window.addEventListener(NAV_EVENT, fn);
  return () => {
    window.removeEventListener("popstate", fn);
    window.removeEventListener(NAV_EVENT, fn);
  };
}

/** One URL param, re-rendering on navigate, Back and Forward. */
export function useNavParam(key: NavKey): string | null {
  return useSyncExternalStore(subscribe, () => readNavParam(key), () => null);
}

function applyNav(url: URL, patch: Nav): void {
  const q = url.searchParams;
  const put = (key: NavKey, value: string | null | undefined) => {
    if (value) q.set(key, value);
    else q.delete(key);
  };
  let below = false;
  for (const key of NAV_LEVELS) {
    if (key in patch) {
      const next = patch[key] || null;
      if (next !== (q.get(key) || null)) below = true;
      put(key, next);
    } else if (below) {
      q.delete(key);
    }
  }
  if ("board" in patch) put("board", patch.board);
}

/** Moves this window. `replace` corrects the URL without a history entry. */
export function navigate(patch: Nav, opts: { replace?: boolean } = {}): void {
  const url = new URL(window.location.href);
  applyNav(url, patch);
  if (url.href === window.location.href) return;
  if (opts.replace) window.history.replaceState(window.history.state, "", url);
  else window.history.pushState(null, "", url);
  saveLastPlace();
  window.dispatchEvent(new Event(NAV_EVENT));
}

/*
 * The last place, for the next cold start: frame, workspace, Controller pane and an open board. Not the Setup step:
 * a restart should not land in the middle of a calibration.
 */
const PLACE_KEY = "arduloops.nav.v1";
const PLACE_FRAMES = ["home", "copter", "plane"];
const PLACE_WS = ["loops", "controller", "mission", "lua", "audit"];
const PLACE_PANES = ["setup", "logs", "params", "firmware", "chats"];
type Place = { frame?: string; ws?: string; pane?: string; board?: string };

/** The board a cold start put back, until the board list says whether it still exists (`dropRestoredBoard`). */
let restoredBoard: string | null = null;

/** Keeps only values a screen can show; anything else is left out, so that level opens on its default. */
function cleanPlace(raw: unknown): Place {
  if (!raw || typeof raw !== "object") return {};
  const r = raw as Record<string, unknown>;
  const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);
  const out: Place = {};
  const frame = str(r.frame);
  if (frame && PLACE_FRAMES.includes(frame)) out.frame = frame;
  const ws = str(r.ws) ? wsParam(WS_ID[str(r.ws)!] ?? str(r.ws)!) : undefined;
  if (out.frame && out.frame !== "home" && ws && PLACE_WS.includes(ws)) out.ws = ws;
  const pane = str(r.pane);
  if (out.ws === "controller" && pane && PLACE_PANES.includes(pane)) out.pane = pane;
  const board = str(r.board);
  if (board && board.length <= 200) out.board = board;
  return out;
}

function writePlace(place: Place): void {
  try {
    localStorage.setItem(PLACE_KEY, JSON.stringify(place));
  } catch {
    /* ignore quota / private mode */
  }
}

function saveLastPlace(): void {
  if (isDetachedWindow()) return;
  const q = search();
  writePlace(cleanPlace({ frame: q.get("frame"), ws: q.get("ws"), pane: q.get("pane"), board: q.get("board") }));
}

function loadPlace(): Place {
  try {
    return cleanPlace(JSON.parse(localStorage.getItem(PLACE_KEY) || "{}"));
  } catch {
    return {};
  }
}

/**
 * Main window, before the first render. A URL without nav params opens on the last place (replaceState, so Back
 * does not step to an empty Home). A URL with params, and a deep link that arrives later, win over it.
 * Back and Forward also update the saved place.
 */
export function startNavMemory(): void {
  if (typeof window === "undefined" || isDetachedWindow()) return;
  window.addEventListener("popstate", saveLastPlace);
  const q = search();
  if (NAV_KEYS.some((key) => q.has(key))) {
    saveLastPlace();
    return;
  }
  const place = loadPlace();
  // A stored value that no longer passes is replaced by what is kept.
  writePlace(place);
  if (!Object.keys(place).length) return;
  const url = new URL(window.location.href);
  for (const key of ["frame", "ws", "pane", "board"] as const) {
    const v = place[key];
    if (v) url.searchParams.set(key, v);
  }
  window.history.replaceState(window.history.state, "", url);
  restoredBoard = place.board ?? null;
}

/** The board list arrived: a restored board that is no longer in it closes, and the saved place forgets it. */
export function dropRestoredBoard(known: readonly string[]): void {
  const board = restoredBoard;
  restoredBoard = null;
  if (!board || known.includes(board) || readNavParam("board") !== board) return;
  navigate({ board: null }, { replace: true });
}

/** A relative href for a place, for an `<a>` that also works with a middle click. */
export function navHref(patch: Nav): string {
  const url = new URL(window.location.href);
  applyNav(url, patch);
  return `${url.pathname}${url.search}`;
}

/**
 * The place an app link names, or null when the href is not one.
 * Levels above the deepest one given keep their value; levels below it reset to the screen default.
 */
export function appLinkNav(href: string): Nav | null {
  let url: URL;
  try {
    url = new URL(href, window.location.href);
  } catch {
    return null;
  }
  if (url.protocol === APP_SCHEME) {
    // `arduloops://open?…` parses as host "open"; `arduloops:open?…` as path "open". Both are the same link.
    const verb = (url.hostname || url.pathname).replace(/^\/+|\/+$/g, "");
    if (verb && verb !== "open") return null;
  } else if (!href.includes("?") || url.origin !== window.location.origin || url.pathname !== window.location.pathname) {
    // A relative link carries its own query; `#x` or `/` would only repeat the current one.
    return null;
  }
  const q = url.searchParams;
  if (!NAV_KEYS.some((key) => q.has(key))) return null;
  const nav: Nav = {};
  let deepest = -1;
  NAV_LEVELS.forEach((key, i) => {
    if (q.has(key)) deepest = i;
  });
  NAV_LEVELS.forEach((key, i) => {
    if (q.has(key)) {
      const raw = (q.get(key) || "").trim().toLowerCase();
      nav[key] = key === "ws" ? wsParam(raw) : raw;
    } else if (i < deepest) {
      return;
    } else {
      nav[key] = null;
    }
  });
  // A link to a place in the vehicle screens leaves a board opened from the database.
  nav.board = q.get("board") || null;
  return nav;
}

/**
 * Opens an app link. In a detached window the main window moves. False when the href is not an app link.
 */
export function openAppLink(href: string, opts: { replace?: boolean } = {}): boolean {
  const nav = appLinkNav(href);
  if (!nav) return false;
  if (isDetachedWindow()) postWindowMsg({ type: "navigate", nav });
  else navigate(nav, opts);
  return true;
}

/** The main window follows app links clicked in detached windows. */
export function followDetachedNav(): () => void {
  return onWindowMsg((msg) => {
    if (msg.type !== "navigate" || !msg.nav || typeof msg.nav !== "object") return;
    const nav: Nav = {};
    for (const key of NAV_KEYS) {
      const v = (msg.nav as Record<string, unknown>)[key];
      if (v === null || typeof v === "string") nav[key] = v;
    }
    navigate(nav);
    window.focus();
  });
}

/** The main window follows arduloops:// links the OS hands to the running desktop app. */
export function followOsLinks(): () => void {
  if (!isTauri()) return () => {};
  let stop: (() => void) | null = null;
  let gone = false;
  void import("@tauri-apps/api/event")
    .then(({ listen }) => listen<string>("app-link", (ev) => {
      if (typeof ev.payload === "string") openAppLink(ev.payload);
    }))
    .then(async (unlisten) => {
      if (gone) return unlisten();
      stop = unlisten;
      // The link that started the app: the first screen becomes that place, without a Back step to the default.
      const { invoke } = await import("@tauri-apps/api/core");
      const first = await invoke<string | null>("take_app_link");
      if (!gone && first) openAppLink(first, { replace: true });
    })
    .catch(() => {});
  return () => {
    gone = true;
    stop?.();
  };
}
