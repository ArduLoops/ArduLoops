// The mission plan: points with a navigation kind, the actions and conditions that
// follow each point, and how that becomes ArduPilot mission items.
//
// ArduPilot semantics this follows (AP_Mission::advance_current_nav_cmd):
// - DO and CONDITION items after point N start when point N is reached, while the
//   vehicle flies on to the next point. They do not stop the vehicle.
// - A CONDITION holds the actions after it (until the next point) until it is met.
//   CONDITION_DISTANCE counts down the distance to the next point.
// - DO_JUMP sends the mission back (or forward) to a point, a number of times.
// - To wait at a place, the point itself loiters: LOITER_TIME or LOITER_TURNS.

import type { MissionItem } from "../mav/cmd";

export type LatLon = { lat: number; lon: number };
export type Finish = "rtl" | "loiter";
export type PointKind = "wp" | "time" | "turns" | "land";
export type ActKind =
  | "speed"
  | "servo"
  | "relay"
  | "cam_dist"
  | "jump"
  | "roi"
  | "land_start"
  | "delay"
  | "distance"
  | "yaw";

/** `a` and `b` mean what ACTS says for the kind. Point references are 0-based indices. */
export type Act = { kind: ActKind; a: number; b: number };
export type Pt = LatLon & { kind?: PointKind; hold?: number; acts?: Act[] };
export type Plan = { points: Pt[]; alt: number; finish: Finish };
export type Vehicle = "copter" | "plane";

export const MAX_POINTS = 40;
export const MAX_ACTS = 8;

type Field = { label: string; min: number; max: number; step: number; def: number; point?: boolean };
export type ActSpec = {
  /** English source phrase for the menu and the list. */
  label: string;
  cond?: boolean;
  only?: Vehicle;
  a?: Field;
  b?: Field;
};

export const ACTS: Record<ActKind, ActSpec> = {
  speed: { label: "Change speed", a: { label: "m/s", min: 1, max: 60, step: 1, def: 15 } },
  servo: {
    label: "Set servo",
    a: { label: "channel", min: 1, max: 16, step: 1, def: 9 },
    b: { label: "PWM", min: 800, max: 2200, step: 10, def: 1900 },
  },
  relay: {
    label: "Set relay",
    a: { label: "relay", min: 0, max: 5, step: 1, def: 0 },
    b: { label: "on", min: 0, max: 1, step: 1, def: 1 },
  },
  cam_dist: { label: "Camera every", a: { label: "m", min: 0, max: 500, step: 5, def: 50 } },
  jump: {
    label: "Jump to point",
    a: { label: "point", min: 0, max: MAX_POINTS - 1, step: 1, def: 0, point: true },
    b: { label: "times", min: -1, max: 20, step: 1, def: 2 },
  },
  roi: { label: "Look at point", only: "copter", a: { label: "point", min: 0, max: MAX_POINTS - 1, step: 1, def: 0, point: true } },
  land_start: { label: "Landing starts here", only: "plane" },
  delay: { label: "Wait", cond: true, a: { label: "s", min: 0, max: 600, step: 1, def: 10 } },
  distance: { label: "Wait until near next point", cond: true, a: { label: "m", min: 0, max: 2000, step: 10, def: 100 } },
  yaw: { label: "Wait for heading", cond: true, only: "copter", a: { label: "deg", min: 0, max: 359, step: 5, def: 0 } },
};

export const POINT_KINDS: Record<PointKind, { label: string; hold?: Field }> = {
  wp: { label: "Waypoint" },
  time: { label: "Circle for time", hold: { label: "s", min: 1, max: 3600, step: 5, def: 30 } },
  turns: { label: "Circle turns", hold: { label: "turns", min: 1, max: 100, step: 1, def: 2 } },
  land: { label: "Land" },
};

export function actsFor(vehicle: Vehicle): ActKind[] {
  return (Object.keys(ACTS) as ActKind[]).filter((k) => !ACTS[k].only || ACTS[k].only === vehicle);
}

function clampField(v: unknown, f: Field | undefined): number {
  if (!f) return 0;
  const n = Number(v);
  if (!Number.isFinite(n)) return f.def;
  return Math.min(f.max, Math.max(f.min, f.step >= 1 ? Math.round(n) : n));
}

export function newAct(kind: ActKind, point: number): Act {
  const spec = ACTS[kind];
  // A jump goes back to the first point by default; a ROI looks at the next one.
  const a = spec.a?.point ? (kind === "roi" ? point + 1 : 0) : (spec.a?.def ?? 0);
  return { kind, a, b: spec.b?.def ?? 0 };
}

/** Clean a point read from storage or the board: known kinds, values inside the limits. */
export function cleanPoint(raw: unknown, vehicle: Vehicle): Pt | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const lat = Number(r.lat);
  const lon = Number(r.lon);
  if (!Number.isFinite(lat) || !Number.isFinite(lon) || Math.abs(lat) > 90 || Math.abs(lon) > 180) return null;
  const kind: PointKind = r.kind === "time" || r.kind === "turns" || r.kind === "land" ? r.kind : "wp";
  const pt: Pt = { lat, lon };
  if (kind !== "wp") pt.kind = kind;
  const hold = POINT_KINDS[kind].hold;
  if (hold) pt.hold = clampField(r.hold, hold);
  const acts = Array.isArray(r.acts)
    ? (r.acts as unknown[])
        .map((x) => {
          const o = x as Record<string, unknown>;
          const k = o?.kind as ActKind;
          if (!ACTS[k] || (ACTS[k].only && ACTS[k].only !== vehicle)) return null;
          return { kind: k, a: clampField(o.a, ACTS[k].a), b: clampField(o.b, ACTS[k].b) } as Act;
        })
        .filter((x): x is Act => x != null)
        .slice(0, MAX_ACTS)
    : [];
  if (acts.length) pt.acts = acts;
  return pt;
}

export type SeqRole =
  | { role: "home" }
  | { role: "takeoff" }
  | { role: "point"; point: number }
  | { role: "act"; point: number; act: number }
  | { role: "rtl" };

export type Built = { items: MissionItem[]; roles: SeqRole[]; pointSeq: number[] };

/** Item 0 home, 1 takeoff, then each point followed by its actions, then RTL. */
export function buildMission(origin: LatLon, plan: Plan, vehicle: Vehicle): Built {
  const { points, alt, finish } = plan;
  const pointSeq: number[] = [];
  let seq = 2;
  points.forEach((p) => {
    pointSeq.push(seq);
    seq += 1 + (p.acts?.length ?? 0);
  });
  const items: MissionItem[] = [
    { kind: "waypoint", lat: origin.lat, lon: origin.lon, alt: 0 },
    { kind: "takeoff", lat: origin.lat, lon: origin.lon, alt },
  ];
  const roles: SeqRole[] = [{ role: "home" }, { role: "takeoff" }];
  points.forEach((p, i) => {
    const last = i === points.length - 1;
    const kind = p.kind ?? "wp";
    const nav: MissionItem =
      kind === "time"
        ? { kind: "loiter_time", lat: p.lat, lon: p.lon, alt, p1: p.hold ?? 30 }
        : kind === "turns"
          ? { kind: "loiter_turns", lat: p.lat, lon: p.lon, alt, p1: p.hold ?? 2 }
          : kind === "land"
            ? { kind: "land", lat: p.lat, lon: p.lon, alt: 0 }
            : { kind: finish === "loiter" && last ? "loiter" : "waypoint", lat: p.lat, lon: p.lon, alt };
    items.push(nav);
    roles.push({ role: "point", point: i });
    (p.acts ?? []).forEach((act, j) => {
      roles.push({ role: "act", point: i, act: j });
      items.push(actItem(act, p, points, pointSeq, alt, vehicle));
    });
  });
  if (finish === "loiter" && points.length === 0) {
    items.push({ kind: "loiter", lat: origin.lat, lon: origin.lon, alt });
    roles.push({ role: "point", point: -1 });
  }
  // A mission that ends with a landing never reaches an RTL after it.
  if (finish === "rtl" && points[points.length - 1]?.kind !== "land") {
    items.push({ kind: "rtl", lat: origin.lat, lon: origin.lon, alt: 0 });
    roles.push({ role: "rtl" });
  }
  return { items, roles, pointSeq };
}

function actItem(act: Act, at: Pt, points: Pt[], pointSeq: number[], alt: number, vehicle: Vehicle): MissionItem {
  const base = { lat: 0, lon: 0, alt: 0 };
  switch (act.kind) {
    case "speed":
      // Plane: airspeed (0). Copter: ground speed (1); both set the horizontal speed.
      return { ...base, kind: "speed", p1: vehicle === "plane" ? 0 : 1, p2: act.a };
    case "servo":
      return { ...base, kind: "servo", p1: act.a, p2: act.b };
    case "relay":
      return { ...base, kind: "relay", p1: act.a, p2: act.b };
    case "cam_dist":
      return { ...base, kind: "cam_dist", p1: act.a };
    case "jump": {
      const target = pointSeq[Math.min(Math.max(0, act.a), pointSeq.length - 1)] ?? 2;
      return { ...base, kind: "jump", p1: target, p2: act.b };
    }
    case "roi": {
      const p = points[Math.min(Math.max(0, act.a), points.length - 1)] ?? at;
      return { kind: "roi", lat: p.lat, lon: p.lon, alt: 0 };
    }
    case "land_start":
      return { kind: "land_start", lat: at.lat, lon: at.lon, alt };
    case "delay":
      return { ...base, kind: "delay", p1: act.a };
    case "distance":
      return { ...base, kind: "distance", p1: act.a };
    case "yaw":
      return { ...base, kind: "yaw", p1: act.a };
  }
}

/** MAV_CMD ids, as MISSION_ITEM_INT.command carries them. */
export const CMD_ID: Record<MissionItem["kind"], number> = {
  waypoint: 16,
  loiter: 17,
  loiter_turns: 18,
  loiter_time: 19,
  rtl: 20,
  land: 21,
  takeoff: 22,
  delay: 112,
  distance: 114,
  yaw: 115,
  jump: 177,
  speed: 178,
  relay: 181,
  servo: 183,
  land_start: 189,
  roi: 201,
  cam_dist: 206,
};

export type BoardItem = {
  seq: number;
  cmd: number;
  frame: number;
  p1: number;
  p2: number;
  p3: number;
  p4: number;
  lat: number;
  lon: number;
  alt: number;
};

const POSITIONED = new Set(["waypoint", "loiter", "loiter_time", "loiter_turns", "land", "takeoff", "roi", "land_start"]);

/** Whether the board holds this mission. Item 0 (home) is the board's own and is skipped. */
export function sameMission(built: MissionItem[], board: BoardItem[]): boolean {
  if (built.length !== board.length) return false;
  for (let i = 1; i < built.length; i++) {
    const want = built[i];
    const got = board[i];
    if (CMD_ID[want.kind] !== got.cmd) return false;
    if (POSITIONED.has(want.kind) && want.kind !== "takeoff") {
      if (Math.abs(Math.round(want.lat * 1e7) - Math.round(got.lat * 1e7)) > 1) return false;
      if (Math.abs(Math.round(want.lon * 1e7) - Math.round(got.lon * 1e7)) > 1) return false;
    }
    if (want.kind !== "rtl" && want.kind !== "roi" && Math.abs(want.alt - got.alt) > 0.05 && POSITIONED.has(want.kind)) return false;
    if (Math.abs((want.p1 ?? 0) - got.p1) > 0.01) return false;
    if (Math.abs((want.p2 ?? 0) - got.p2) > 0.01) return false;
  }
  return true;
}

/** Turn a board mission back into a plan. Items this screen does not edit are counted, not kept. */
export function boardToPlan(board: BoardItem[], vehicle: Vehicle, fallbackAlt: number): { plan: Plan; skipped: number } {
  const points: Pt[] = [];
  let finish: Finish = "loiter";
  let alt = fallbackAlt;
  let skipped = 0;
  const seqPoint = new Map<number, number>();
  const pending: { point: number; act: Act; target?: number; at?: LatLon }[] = [];
  for (const it of board.slice(1)) {
    const here = points.length - 1;
    switch (it.cmd) {
      case 22:
        if (it.alt > 0) alt = it.alt;
        break;
      case 16:
      case 17:
      case 18:
      case 19:
      case 21: {
        const p: Pt = { lat: it.lat, lon: it.lon };
        if (it.cmd === 19) Object.assign(p, { kind: "time", hold: it.p1 });
        if (it.cmd === 18) Object.assign(p, { kind: "turns", hold: it.p1 });
        if (it.cmd === 21) p.kind = "land";
        if (it.cmd !== 21 && points.length === 0 && it.alt > 0) alt = it.alt;
        seqPoint.set(it.seq, points.length);
        points.push(p);
        finish = it.cmd === 17 ? "loiter" : finish;
        break;
      }
      case 20:
        finish = "rtl";
        break;
      default: {
        if (here < 0) {
          skipped++;
          break;
        }
        const map: Record<number, () => Act | null> = {
          178: () => ({ kind: "speed", a: it.p2, b: 0 }),
          183: () => ({ kind: "servo", a: it.p1, b: it.p2 }),
          181: () => ({ kind: "relay", a: it.p1, b: it.p2 }),
          206: () => ({ kind: "cam_dist", a: it.p1, b: 0 }),
          177: () => ({ kind: "jump", a: 0, b: it.p2 }),
          201: () => ({ kind: "roi", a: 0, b: 0 }),
          189: () => ({ kind: "land_start", a: 0, b: 0 }),
          112: () => ({ kind: "delay", a: it.p1, b: 0 }),
          114: () => ({ kind: "distance", a: it.p1, b: 0 }),
          115: () => ({ kind: "yaw", a: it.p1, b: 0 }),
        };
        const act = map[it.cmd]?.() ?? null;
        if (!act || (ACTS[act.kind].only && ACTS[act.kind].only !== vehicle)) {
          skipped++;
          break;
        }
        pending.push({ point: here, act, target: it.cmd === 177 ? it.p1 : undefined, at: it.cmd === 201 ? it : undefined });
      }
    }
  }
  // The last loiter-unlimited point means the plan ends with a loiter; a plain last waypoint with no RTL does too.
  for (const p of pending) {
    if (p.act.kind === "jump") p.act.a = seqPoint.get(p.target ?? -1) ?? 0;
    if (p.act.kind === "roi" && p.at) {
      let best = 0;
      let bestD = Infinity;
      points.forEach((q, i) => {
        const d = Math.hypot(q.lat - p.at!.lat, q.lon - p.at!.lon);
        if (d < bestD) {
          bestD = d;
          best = i;
        }
      });
      p.act.a = best;
    }
    const pt = points[p.point];
    pt.acts = [...(pt.acts ?? []), p.act].slice(0, MAX_ACTS);
  }
  const clean = points.map((p) => cleanPoint(p, vehicle)).filter((p): p is Pt => p != null).slice(0, MAX_POINTS);
  return { plan: { points: clean, alt, finish }, skipped };
}

/**
 * ArduPlane keeps CONDITION_DELAY and CONDITION_DISTANCE in condition_value, and starting a
 * LOITER_TIME or LOITER_TURNS point sets condition_value = 1 (commands_logic.cpp). A condition
 * after point i is overwritten when the vehicle heads next for such a circle: a distance then
 * never holds, a delay ends at once, and the actions after it may never run.
 */
/** Whether the plan flies home at the end: RTL is on and the last point is not a landing. */
export function endsHome(plan: Plan): boolean {
  return plan.finish === "rtl" && plan.points[plan.points.length - 1]?.kind !== "land";
}

export function conditionClobbered(plan: Plan, i: number, vehicle: Vehicle): boolean {
  if (vehicle !== "plane") return false;
  const acts = plan.points[i]?.acts ?? [];
  if (!acts.some((a) => a.kind === "delay" || a.kind === "distance")) return false;
  const circles = (k: number) => {
    const kind = plan.points[k]?.kind;
    return kind === "time" || kind === "turns";
  };
  const jump = acts.find((a) => a.kind === "jump");
  return (jump != null && jump.b !== 0 && circles(jump.a)) || circles(i + 1);
}
