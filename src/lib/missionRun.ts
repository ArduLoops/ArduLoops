// What the running mission has done, read from ArduPilot's own reports.
//
// AP_Mission::start_command sends "Mission: <seq> <name>" for every command it starts,
// navigation and DO alike; a DO_JUMP reports "Mission: <seq> Jump n/m". The times
// those texts arrive give, per item, when it last started.

import { getSnapshot, subscribe } from "../mav/store";
import type { Sample } from "../mav/types";

const started = new Map<number, number>();
const jumps = new Map<number, string>();
let lastTexts: string[] = [];
let hooked = false;
const listeners = new Set<(seq: number, name: string) => void>();

const RE = /^\S+ Mission: (\d+) (\S+)(?: (.*))?$/;

// Texts arrive newest first. New ones are those in front of the previous head.
function freshTexts(texts: string[]): string[] {
  if (!lastTexts.length) return [];
  const head = lastTexts.slice(0, 6);
  for (let k = 0; k <= Math.min(texts.length, 64); k++) {
    let match = true;
    for (let m = 0; m < head.length && k + m < texts.length; m++) {
      if (texts[k + m] !== head[m]) {
        match = false;
        break;
      }
    }
    if (match) return texts.slice(0, k);
  }
  return texts.slice(0, 8);
}

function note(s: Sample): void {
  const texts = s.texts ?? [];
  if (texts === lastTexts) return;
  const fresh = lastTexts.length ? freshTexts(texts) : [];
  lastTexts = texts;
  const now = Date.now();
  for (const line of fresh.reverse()) {
    const m = RE.exec(line);
    if (!m) continue;
    const seq = Number(m[1]);
    started.set(seq, now);
    if (m[2] === "Jump" && m[3]) jumps.set(seq, m[3]);
    for (const fn of listeners) fn(seq, m[2]);
  }
}

export function hookMissionRun(): void {
  if (hooked) return;
  hooked = true;
  lastTexts = getSnapshot().texts ?? [];
  subscribe(() => note(getSnapshot()));
}

/** When the item last started, in ms since epoch. */
export function startedAt(seq: number): number | null {
  return started.get(seq) ?? null;
}

/** "n/m" for a DO_JUMP that has run. */
export function jumpCount(seq: number): string | null {
  return jumps.get(seq) ?? null;
}

/** Forget what ran, for a freshly uploaded mission. */
export function resetMissionRun(): void {
  started.clear();
  jumps.clear();
}

export function onMissionItem(fn: (seq: number, name: string) => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}
