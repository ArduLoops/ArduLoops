import { useSyncExternalStore } from "react";
import type { Axis } from "../mav/axis";
import { onWindowMsg, postWindowMsg } from "./detach";

/**
 * What the loops studio looks at: the selected card, the axis, and whether the map shows every loop.
 * One value for the app: the studio, its panes in floating windows, and their detached windows follow it.
 * A change in any window goes to the others over the windows channel (lib/detach.ts).
 */
export type StudioFocus = { sel: string | null; axis: Axis; showAll: boolean };

let focus: StudioFocus = { sel: null, axis: "roll", showAll: false };
const subs = new Set<() => void>();

function apply(next: StudioFocus): void {
  if (next.sel === focus.sel && next.axis === focus.axis && next.showAll === focus.showAll) return;
  focus = next;
  for (const fn of subs) fn();
}

export function getStudioFocus(): StudioFocus {
  return focus;
}

/** A part of the focus, or a function of the current one. */
export function setStudioFocus(change: Partial<StudioFocus> | ((cur: StudioFocus) => Partial<StudioFocus>)): void {
  const part = typeof change === "function" ? change(focus) : change;
  const next = { ...focus, ...part };
  if (next.sel === focus.sel && next.axis === focus.axis && next.showAll === focus.showAll) return;
  apply(next);
  postWindowMsg({ type: "studio-focus", focus: next });
}

export function useStudioFocus(): StudioFocus {
  return useSyncExternalStore(
    (fn) => {
      subs.add(fn);
      return () => subs.delete(fn);
    },
    () => focus,
  );
}

onWindowMsg((msg) => {
  if (msg.type === "studio-focus") apply(msg.focus as StudioFocus);
  else if (msg.type === "sync-req") postWindowMsg({ type: "studio-focus", focus });
});

/** Copter: a card picked on the map or in a list, and the axis it belongs to. */
export function copterPick(next: string | null): void {
  setStudioFocus((cur) => {
    let axis = cur.axis;
    if (next?.startsWith("psc_d")) axis = "d";
    else if (next === "psc_ne_pos" || next === "psc_ne_vel" || next === "lean") axis = axis === "d" || axis === "yaw" ? "roll" : axis;
    else if (next === "atc_ang" || next === "atc_rat") axis = axis === "d" ? "roll" : axis;
    return { sel: next, axis };
  });
}

/** Copter: a new axis, and the card that stays selected on it. */
export function copterAxis(next: Axis): void {
  setStudioFocus((cur) => {
    const id = cur.sel;
    let sel = id;
    if (next === "d") sel = id?.startsWith("psc_d") || id === "pilot" || id === "nav" || id === "motors" ? id : "psc_d_pos";
    else if (id?.startsWith("psc_d")) sel = "atc_rat";
    return { axis: next, sel };
  });
}

/** Plane: a card picked on the map, and the axis it belongs to. */
export function planePick(next: string | null): void {
  setStudioFocus((cur) => {
    let axis = cur.axis;
    if (next === "ptch_ang" || next === "ptch_rate" || next === "elevator") axis = "pitch";
    else if (next === "yaw_damp" || next === "rudder") axis = "yaw";
    else if (next === "rll_ang" || next === "rll_rate" || next === "aileron" || next === "ahrs") axis = "roll";
    return { sel: next, axis };
  });
}

/** Plane: a new axis selects that axis's rate loop. */
export function planeAxis(next: Axis): void {
  setStudioFocus({ axis: next, sel: next === "pitch" ? "ptch_rate" : next === "yaw" ? "yaw_damp" : "rll_rate" });
}

/** A pane of the loops studio. */
export type StudioPane = "scheme" | "loop" | "scope";

// Panes in floating windows in this window of the app (components/StudioWindows.tsx). The studio shows a note in their place.
let popped: StudioPane[] = [];
const poppedSubs = new Set<() => void>();

function setPopped(next: StudioPane[]): void {
  popped = next;
  for (const fn of poppedSubs) fn();
}

export function popStudioPane(pane: StudioPane): void {
  if (!popped.includes(pane)) setPopped([...popped, pane]);
}

export function returnStudioPane(pane: StudioPane): void {
  setPopped(popped.filter((p) => p !== pane));
}

export function usePoppedPanes(): StudioPane[] {
  return useSyncExternalStore(
    (fn) => {
      poppedSubs.add(fn);
      return () => poppedSubs.delete(fn);
    },
    () => popped,
  );
}
