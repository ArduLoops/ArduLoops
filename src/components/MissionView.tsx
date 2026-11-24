import "./MissionView.css";
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactElement } from "react";
import { useT } from "../i18n/i18n";
import { addLog } from "../log";
import { noteUi, readMission, send, uploadMission } from "../mav/cmd";
import { getSnapshot, subscribe } from "../mav/store";
import type { Sample } from "../mav/types";
import { usePicked, useVehicle } from "../mav/view";
import { setScreenView, type MissionDetail } from "../lib/setupView";
import { WorkspaceTabs, type WorkspaceId } from "./WorkspaceTabs";
import { MissionEditor, actBadge, actText, type ActState } from "./MissionEditor";
import {
  ACTS,
  MAX_POINTS,
  boardToPlan,
  buildMission,
  cleanPoint,
  endsHome,
  sameMission,
  type BoardItem,
  type Built,
  type Finish,
  type LatLon,
  type Plan,
  type Pt,
  type SeqRole,
} from "../lib/missionPlan";
import { hookMissionRun, onMissionItem, resetMissionRun, startedAt } from "../lib/missionRun";

const M_PER_DEG = 111_320;
const TRAIL_CAP = 700;

type View = { lat: number; lon: number; mpp: number };
type Pose = {
  ok: boolean;
  mode: string;
  lat: number | null;
  lon: number | null;
  gpsLat: number | null;
  gpsLon: number | null;
  truthLat: number | null;
  truthLon: number | null;
  homeLat: number | null;
  homeLon: number | null;
  alt: number | null;
  gspd: number | null;
  hdg: number | null;
  yaw: number;
  sats: number | null;
  wpSeq: number | null;
};

const KNOWN = new Set([
  "Mission uploaded",
  "Mission cleared",
  "Mission upload timed out",
  "No link",
  "Bad mission",
  "Too many waypoints",
  "Mission rejected",
  "Connection lost during mission upload",
  "Mission download timed out",
  "Connection lost during mission download",
]);

type TrailPt = LatLon & { truth: LatLon | null; gps: LatLon | null };
const trail: TrailPt[] = [];
let trailFrame = "";
let trailHooked = false;

function isLat(p: unknown): p is LatLon {
  if (!p || typeof p !== "object") return false;
  const lat = (p as LatLon).lat;
  const lon = (p as LatLon).lon;
  return Number.isFinite(lat) && Number.isFinite(lon) && Math.abs(lat) <= 90 && Math.abs(lon) <= 180;
}

function toEnu(lat: number, lon: number, origin: LatLon): { e: number; n: number } {
  const north = (lat - origin.lat) * M_PER_DEG;
  const east = (lon - origin.lon) * M_PER_DEG * Math.cos((origin.lat * Math.PI) / 180);
  return { e: east, n: north };
}

function fromEnu(east: number, north: number, origin: LatLon): LatLon {
  const lat = origin.lat + north / M_PER_DEG;
  const cos = Math.cos((origin.lat * Math.PI) / 180) || 1e-6;
  const lon = origin.lon + east / (M_PER_DEG * cos);
  return { lat, lon };
}

function dist(a: LatLon, b: LatLon): number {
  const d = toEnu(b.lat, b.lon, a);
  return Math.hypot(d.e, d.n);
}

/** The plan in numbers for the assistant: legs from home, the sharpest turn, no coordinates. */
function missionDetail(plan: Plan, origin: LatLon | null, vehicle: string, pose: Pose, params: Record<string, number | undefined>): MissionDetail {
  const path = origin ? [origin, ...plan.points, ...(plan.finish === "rtl" ? [origin] : [])] : plan.points;
  const legs = path.slice(1).map((p, i) => Math.round(dist(path[i], p)));
  let sharpest: number | null = null;
  for (let i = 1; i < path.length - 1; i++) {
    const a = toEnu(path[i].lat, path[i].lon, path[i - 1]);
    const b = toEnu(path[i + 1].lat, path[i + 1].lon, path[i]);
    if (Math.hypot(a.e, a.n) < 1 || Math.hypot(b.e, b.n) < 1) continue;
    const turn = Math.abs(wrap180((Math.atan2(b.e, b.n) - Math.atan2(a.e, a.n)) * (180 / Math.PI)));
    sharpest = Math.max(sharpest ?? 0, Math.round(turn));
  }
  const read = (name: string) => (params[name] != null && Number.isFinite(params[name]) ? (params[name] as number) : null);
  return {
    points: plan.points.length,
    actions: plan.points.reduce((n, p) => n + (p.acts?.length ?? 0), 0),
    alt_m: plan.alt,
    finish: plan.finish,
    legs_m: legs.slice(0, 40),
    total_m: legs.reduce((sum, leg) => sum + leg, 0),
    sharpest_turn_deg: sharpest,
    ...(vehicle === "plane"
      ? { nav: { NAVL1_PERIOD: read("NAVL1_PERIOD"), NAVL1_DAMPING: read("NAVL1_DAMPING"), WP_RADIUS: read("WP_RADIUS"), AIRSPEED_CRUISE: read("AIRSPEED_CRUISE") } }
      : {}),
    mode: pose.mode,
    current_wp: pose.wpSeq,
  };
}

const L1_INV_PI = 0.3183099;

function wrap180(deg: number): number {
  return ((deg + 180) % 360 + 360) % 360 - 180;
}

function posNum(v: number | undefined, fallback: number): number {
  return v != null && Number.isFinite(v) && v > 0 ? v : fallback;
}

type L1Tune = {
  period: number;
  damp: number;
  wpRadius: number;
  cruise: number;
  /** WP_LOITER_RAD, signed: negative loiters counter-clockwise. */
  loiterRad: number;
  /** RTL_RADIUS, signed; |value| <= 1 falls back to WP_LOITER_RAD. */
  rtlRad: number;
  rollLimit: number;
  /** NAVL1_LIM_BANK; 0 scales loiter radii by EAS2TAS squared only. */
  limBank: number;
};

function anyNum(v: number | undefined, fallback: number): number {
  return v != null && Number.isFinite(v) ? v : fallback;
}

function pickTune(s: Sample): L1Tune {
  return {
    period: posNum(s.params.NAVL1_PERIOD, 17),
    damp: posNum(s.params.NAVL1_DAMPING, 0.75),
    wpRadius: posNum(s.params.WP_RADIUS, 90),
    cruise: posNum(s.params.AIRSPEED_CRUISE, 15),
    loiterRad: anyNum(s.params.WP_LOITER_RAD, 60),
    rtlRad: anyNum(s.params.RTL_RADIUS, 0),
    rollLimit: posNum(s.params.ROLL_LIMIT_DEG, 45),
    limBank: anyNum(s.params.NAVL1_LIM_BANK, 0),
  };
}

function tuneEq(a: L1Tune, b: L1Tune): boolean {
  return (
    a.period === b.period &&
    a.damp === b.damp &&
    a.wpRadius === b.wpRadius &&
    a.cruise === b.cruise &&
    a.loiterRad === b.loiterRad &&
    a.rtlRad === b.rtlRad &&
    a.rollLimit === b.rollLimit &&
    a.limBank === b.limBank
  );
}

type L1Picture = {
  /** line: AP_L1_Control::update_waypoint. capture / circle: the two branches of update_loiter. */
  phase: "line" | "capture" | "circle";
  at: LatLon;
  a: LatLon;
  b: LatLon;
  ref: LatLon;
  foot: LatLon | null;
  along: boolean;
  l1m: number;
  /** Line: cross-track error (+ left of track). Loiter: distance outside the circle. */
  xtrack: number;
  nuDeg: number;
  courseDeg: number;
  leadM: number | null;
  preview: boolean;
  circle: { center: LatLon; r: number } | null;
  /** nav_roll this picture asks for: atan(latAccDem / g), clipped to ROLL_LIMIT_DEG. */
  rollDeg: number;
};

const GRAVITY = 9.80665;

// AP_Baro EAS2TAS from the standard atmosphere at the plane's altitude above sea level.
function eas2tasAt(amsl: number): number {
  const ratio = Math.max(0.2, (288.15 - 0.0065 * Math.max(-500, Math.min(11000, amsl))) / 288.15);
  return 1 / Math.sqrt(ratio ** 4.2559);
}

// AP_L1_Control::loiter_radius: the radius the controller really flies.
function loiterRadius(r: number, tune: L1Tune, e2t: number): number {
  const e2 = e2t * e2t;
  const lim = Math.max(0, Math.min(89, tune.limBank));
  const acc = Math.tan((lim * Math.PI) / 180) * GRAVITY;
  if (lim === 0 || acc === 0) return r * e2;
  const seaLevel = (tune.cruise * tune.cruise) / acc;
  return seaLevel > r ? r * e2 : Math.max(seaLevel * e2, r);
}

function bankFor(latAcc: number, tune: L1Tune): number {
  const deg = (Math.atan(latAcc / GRAVITY) * 180) / Math.PI;
  return Math.max(-tune.rollLimit, Math.min(tune.rollLimit, deg));
}

// Course over ground from the trail. A chord gives the tangent at its middle, so in a
// turn it lags; a second chord behind it gives the turn rate to carry it forward to the
// plane. Without that, a steady loiter reads as drifting outward and the circle bank
// comes out about 2 degrees high.
function motionCourse(board: LatLon, yawDeg: number, hdg: number | null, moving: boolean): number {
  for (let i = trail.length - 1; i >= 0; i--) {
    const d = toEnu(board.lat, board.lon, trail[i]);
    const la = Math.hypot(d.e, d.n);
    if (la < 3) continue;
    const ca = Math.atan2(d.e, d.n);
    for (let j = i - 1; j >= 0; j--) {
      const p = toEnu(trail[i].lat, trail[i].lon, trail[j]);
      const lb = Math.hypot(p.e, p.n);
      if (lb < 3) continue;
      if (lb > 4 * la) break;
      const turn = (wrap180(((ca - Math.atan2(p.e, p.n)) * 180) / Math.PI) * Math.PI) / 180;
      return ca + (turn * la) / (la + lb);
    }
    return ca;
  }
  if (moving && hdg != null && Number.isFinite(hdg)) return (hdg * Math.PI) / 180;
  return (yawDeg * Math.PI) / 180;
}

// The ground-speed vector the controller works with. Standing still, it is a cruise-speed
// preview along the nose so the picture shows what the first moments of flight will do.
function groundVector(
  board: LatLon,
  yawDeg: number,
  hdg: number | null,
  gspd: number | null,
  tune: L1Tune,
): { speed: number; gsE: number; gsN: number; preview: boolean } {
  const yaw = (yawDeg * Math.PI) / 180;
  if ((gspd ?? 0) < 1) {
    return { speed: tune.cruise, gsE: Math.sin(yaw) * tune.cruise, gsN: Math.cos(yaw) * tune.cruise, preview: true };
  }
  const course = motionCourse(board, yawDeg, hdg, true);
  const speed = Math.max(gspd ?? 0, 0);
  return { speed, gsE: Math.sin(course) * speed, gsN: Math.cos(course) * speed, preview: false };
}

// AP_L1_Control::update_waypoint, without the crosstrack integrator.
// NE cross matches Vector2f (x = north, y = east): +xtrack is left of the track.
// turnDeg is the turn onto the next leg; AP_L1_Control::turn_distance shrinks the
// acceptance circle below 90 degrees.
function solveL1(
  board: LatLon,
  a: LatLon,
  b: LatLon,
  yawDeg: number,
  hdg: number | null,
  gspd: number | null,
  tune: L1Tune,
  turnDeg: number | null,
  e2t: number,
): L1Picture | null {
  const ab = toEnu(b.lat, b.lon, a);
  const len = Math.hypot(ab.e, ab.n);
  if (len < 1) return null;
  const abE = ab.e / len;
  const abN = ab.n / len;
  const rel = toEnu(board.lat, board.lon, a);
  const along = rel.n * abN + rel.e * abE;
  const xtrack = rel.n * abE - rel.e * abN;
  const yaw = (yawDeg * Math.PI) / 180;
  const gv = groundVector(board, yawDeg, hdg, gspd, tune);
  let { speed, gsE, gsN } = gv;
  if (!gv.preview && (speed < 0.1 || gsE * Math.sin(yaw) + gsN * Math.cos(yaw) <= 0)) {
    speed = 0.1;
    gsE = Math.sin(yaw) * speed;
    gsN = Math.cos(yaw) * speed;
  }
  const l1m = L1_INV_PI * tune.damp * tune.period * speed;
  if (l1m < 1) return null;
  const wpA = Math.hypot(rel.e, rel.n);
  let ref: LatLon;
  let foot: LatLon | null = null;
  let onLine = false;
  if (wpA > l1m && along / Math.max(wpA, 1) < -0.7071) {
    ref = a;
  } else if (along > len + speed * 3) {
    ref = b;
  } else {
    onLine = true;
    const track = Math.atan2(abE, abN);
    const sine = Math.max(-0.7071, Math.min(0.7071, xtrack / Math.max(l1m, 0.1)));
    const nav = track + Math.asin(sine);
    ref = fromEnu(Math.sin(nav) * l1m, Math.cos(nav) * l1m, board);
    if (Math.abs(xtrack) > 1) foot = fromEnu(along * abE, along * abN, a);
  }
  const courseDeg = (Math.atan2(gsE, gsN) * 180) / Math.PI;
  const toRef = toEnu(ref.lat, ref.lon, board);
  const refM = Math.hypot(toRef.e, toRef.n);
  const nuDeg = refM < 0.5 ? 0 : wrap180((Math.atan2(toRef.e, toRef.n) * 180) / Math.PI - courseDeg);
  const nuRad = (Math.max(-90, Math.min(90, nuDeg)) * Math.PI) / 180;
  const latAcc = ((4 * tune.damp * tune.damp * speed * speed) / l1m) * Math.sin(nuRad);
  const turn = turnDeg == null ? 90 : Math.min(90, Math.abs(turnDeg));
  const lead = (Math.min(tune.wpRadius * e2t * e2t, l1m) * turn) / 90;
  return {
    phase: "line",
    at: board,
    a,
    b,
    ref,
    foot,
    along: onLine,
    l1m,
    xtrack,
    nuDeg,
    courseDeg,
    leadM: onLine && lead >= 1 && len > lead + 8 ? lead : null,
    preview: gv.preview,
    circle: null,
    rollDeg: bankFor(latAcc, tune),
  };
}

// AP_L1_Control::update_loiter. Outside the circle it captures (flies at the centre
// until the circle controller asks for the tighter turn), then it holds the circle.
function solveLoiter(
  board: LatLon,
  center: LatLon,
  r: number,
  dir: 1 | -1,
  yawDeg: number,
  hdg: number | null,
  gspd: number | null,
  tune: L1Tune,
  e2t: number,
): L1Picture {
  r = loiterRadius(r, tune, e2t);
  const gv = groundVector(board, yawDeg, hdg, gspd, tune);
  const speed = Math.max(gv.speed, 1);
  const { gsE, gsN } = gv;
  const omega = (2 * Math.PI) / tune.period;
  const kx = omega * omega;
  const kv = 2 * tune.damp * omega;
  const kL1 = 4 * tune.damp * tune.damp;
  const l1m = L1_INV_PI * tune.damp * tune.period * speed;
  const rel = toEnu(board.lat, board.lon, center);
  const relM = Math.hypot(rel.e, rel.n);
  let uN: number;
  let uE: number;
  if (relM > 0.1) {
    uN = rel.n / relM;
    uE = rel.e / relM;
  } else {
    const yaw = (yawDeg * Math.PI) / 180;
    uN = Math.cos(yaw);
    uE = Math.sin(yaw);
  }
  const xVelCap = uN * gsE - uE * gsN;
  const lVelCap = -(gsN * uN + gsE * uE);
  const nu = Math.atan2(xVelCap, lVelCap);
  const nuC = Math.max(-Math.PI / 2, Math.min(Math.PI / 2, nu));
  const capAcc = ((kL1 * speed * speed) / l1m) * Math.sin(nuC);
  const xVelCirc = -lVelCap;
  const xErr = relM - r;
  let pd = xErr * kx + xVelCirc * kv;
  const velTan = xVelCap * dir;
  const ctr = (velTan * velTan) / Math.max(0.5 * r, r + xErr);
  if (lVelCap < 0) {
    if (velTan < 0) pd = Math.max(pd, 0);
    else if (xErr < 0) pd = Math.max(pd, -ctr);
  }
  const circAcc = dir * (pd + ctr);
  const capture = xErr > 0 && dir * capAcc < dir * circAcc;
  const courseDeg = (Math.atan2(gsE, gsN) * 180) / Math.PI;
  const toCenter = Math.min(relM, l1m);
  const ref = capture ? fromEnu(-uE * toCenter, -uN * toCenter, board) : board;
  return {
    phase: capture ? "capture" : "circle",
    at: board,
    a: center,
    b: center,
    ref,
    foot: null,
    along: false,
    l1m,
    xtrack: xErr,
    nuDeg: capture ? (nu * 180) / Math.PI : 0,
    courseDeg,
    leadM: null,
    preview: gv.preview,
    circle: { center, r },
    rollDeg: bankFor(capture ? capAcc : circAcc, tune),
  };
}

function segmentMiss(a: LatLon, b: LatLon, p: LatLon): number {
  const ab = toEnu(b.lat, b.lon, a);
  const len = Math.hypot(ab.e, ab.n);
  if (len < 1) return Infinity;
  const rel = toEnu(p.lat, p.lon, a);
  const abE = ab.e / len;
  const abN = ab.n / len;
  const along = rel.n * abN + rel.e * abE;
  if (along < 0) return Math.hypot(rel.e, rel.n);
  if (along > len) {
    const end = toEnu(p.lat, p.lon, b);
    return Math.hypot(end.e, end.n);
  }
  return Math.abs(rel.n * abE - rel.e * abN);
}

type NavStep =
  | { kind: "line"; a: LatLon; b: LatLon; turnDeg: number | null }
  | { kind: "loiter"; center: LatLon; r: number; dir: 1 | -1 }
  | { kind: "takeoff" };

function courseTo(a: LatLon, b: LatLon): number {
  const d = toEnu(b.lat, b.lon, a);
  return (Math.atan2(d.e, d.n) * 180) / Math.PI;
}

// Plane::update_loiter: a radius of 1 m or less falls back to WP_LOITER_RAD, whose sign sets the direction.
function loiterOf(radius: number, tune: L1Tune): { r: number; dir: 1 | -1 } {
  if (Math.abs(radius) > 1) return { r: Math.abs(radius), dir: radius < 0 ? -1 : 1 };
  const r = Math.abs(tune.loiterRad) > 1 ? Math.abs(tune.loiterRad) : 60;
  return { r, dir: tune.loiterRad < 0 ? -1 : 1 };
}

// What ArduPlane's navigation runs for the current mode and mission item. `roles` maps
// MISSION_CURRENT (always a navigation item) to the plan: home, takeoff (a course hold,
// no L1 track), a point, or RTL. `direct` marks a leg flown without cross-track (after
// AUTO entry or the takeoff): verify_nav_wp then calls update_waypoint(current_loc,
// next), straight at the point. Otherwise prev_WP is the previous target, which after
// a DO_JUMP is the point the jump left from (`prevTarget`). Loiter points and RTL run
// update_loiter: the line in while cross-tracking and far out, then the circle.
function chooseNav(
  mode: string,
  wpSeq: number | null,
  board: LatLon,
  moving: boolean,
  home: LatLon | null,
  origin: LatLon | null,
  plan: Plan,
  roles: SeqRole[],
  direct: boolean,
  prevTarget: LatLon | null,
  tune: L1Tune,
): NavStep | null {
  const { points, finish } = plan;
  const rtlHome = home ?? origin;
  if (mode === "RTL") {
    if (!rtlHome) return null;
    const { r, dir } = loiterOf(tune.rtlRad, tune);
    return { kind: "loiter", center: rtlHome, r, dir };
  }
  if (!origin) return null;
  if (mode === "AUTO" && wpSeq != null) {
    const role = roles[wpSeq];
    if (!role) return null;
    if (role.role === "takeoff") return { kind: "takeoff" };
    if (role.role === "rtl") {
      if (!rtlHome) return null;
      const { r, dir } = loiterOf(tune.rtlRad, tune);
      return { kind: "loiter", center: rtlHome, r, dir };
    }
    if (role.role !== "point") return null;
    if (role.point < 0) {
      const { r, dir } = loiterOf(0, tune);
      return { kind: "loiter", center: origin, r, dir };
    }
    const i = role.point;
    const target = points[i];
    if (!target) return null;
    const kind = target.kind ?? "wp";
    const a = direct ? board : (prevTarget ?? (i > 0 ? points[i - 1] : origin));
    const loiters = kind === "time" || kind === "turns" || (kind === "wp" && finish === "loiter" && i === points.length - 1);
    if (loiters) {
      const { r, dir } = loiterOf(0, tune);
      if (!direct && dist(board, target) > 3 * r) return { kind: "line", a, b: target, turnDeg: null };
      return { kind: "loiter", center: target, r, dir };
    }
    if (kind === "land") return { kind: "line", a, b: target, turnDeg: null };
    const next = nextAfter(plan, i, origin);
    const turnDeg = next && dist(target, next) > 1 ? wrap180(courseTo(target, next) - courseTo(a, target)) : null;
    return { kind: "line", a, b: target, turnDeg };
  }
  // Other modes do not fly the plan. On the ground, preview the leg nearest the plane.
  if (moving) return null;
  const verts = [origin, ...points];
  const pathVerts = finish === "rtl" && points.length ? [...verts, origin] : verts;
  if (pathVerts.length < 2) return null;
  let bestI = 1;
  let best = Infinity;
  for (let i = 1; i < pathVerts.length; i++) {
    const err = segmentMiss(pathVerts[i - 1], pathVerts[i], board);
    if (err < best) {
      best = err;
      bestI = i;
    }
  }
  return { kind: "line", a: pathVerts[bestI - 1], b: pathVerts[bestI], turnDeg: null };
}

/** The place the vehicle heads for after point i: a jump's target, the next point, or home. */
function nextAfter(plan: Plan, i: number, origin: LatLon): LatLon | null {
  const jump = plan.points[i]?.acts?.find((a) => a.kind === "jump");
  if (jump && plan.points[jump.a]) return plan.points[jump.a];
  if (i + 1 < plan.points.length) return plan.points[i + 1];
  return endsHome(plan) ? origin : null;
}

// Where a point's actions run. A plane counts a waypoint reached on entering the
// acceptance circle (AP_L1_Control::turn_distance), so they run that far before it.
function actionAnchor(plan: Plan, i: number, origin: LatLon, vehicle: string, tune: L1Tune, e2t: number): LatLon {
  const p = plan.points[i];
  if (vehicle !== "plane" || (p.kind ?? "wp") !== "wp") return p;
  const prev = i > 0 ? plan.points[i - 1] : origin;
  const leg = dist(prev, p);
  const next = nextAfter(plan, i, origin);
  const turn = next && dist(p, next) > 1 ? Math.abs(wrap180(courseTo(p, next) - courseTo(prev, p))) : 90;
  const l1 = L1_INV_PI * tune.damp * tune.period * tune.cruise;
  const lead = Math.min(tune.wpRadius * e2t * e2t, l1) * Math.min(90, turn) / 90;
  if (leg < 1 || lead < 1) return p;
  const d = toEnu(prev.lat, prev.lon, p);
  const k = Math.min(lead, leg) / leg;
  return fromEnu(d.e * k, d.n * k, p);
}

function pair(lat: number | null | undefined, lon: number | null | undefined): LatLon | null {
  if (lat == null || lon == null || !Number.isFinite(lat) || !Number.isFinite(lon)) return null;
  if (lat === 0 && lon === 0) return null;
  return { lat, lon };
}

function noteSample(s: Sample): void {
  const board = pair(s.lat, s.lon);
  if (!s.ok || !board) return;
  if (s.frame && trailFrame && s.frame !== trailFrame) trail.length = 0;
  if (s.frame) trailFrame = s.frame;
  const truth = pair(s.truth_lat, s.truth_lon);
  const gps = pair(s.gps_lat, s.gps_lon);
  const last = trail[trail.length - 1];
  if (last) {
    if (dist(last, board) > 2000) trail.length = 0;
    else {
      const boardStill = dist(last, board) < 0.3;
      const truthStill = !truth || !last.truth || dist(last.truth, truth) < 0.3;
      const gpsStill = !gps || !last.gps || dist(last.gps, gps) < 0.3;
      if (boardStill && truthStill && gpsStill) return;
    }
  }
  trail.push({ ...board, truth, gps });
  if (trail.length > TRAIL_CAP) trail.splice(0, trail.length - TRAIL_CAP);
}

function hookTrail(): void {
  if (trailHooked) return;
  trailHooked = true;
  subscribe(() => noteSample(getSnapshot()));
}
hookTrail();

function pickPose(s: Sample): Pose {
  return {
    ok: s.ok,
    mode: s.mode,
    lat: s.lat ?? null,
    lon: s.lon ?? null,
    gpsLat: s.gps_lat ?? null,
    gpsLon: s.gps_lon ?? null,
    truthLat: s.truth_lat ?? null,
    truthLon: s.truth_lon ?? null,
    homeLat: s.home_lat ?? null,
    homeLon: s.home_lon ?? null,
    alt: s.alt,
    gspd: s.gspd,
    hdg: s.hdg,
    yaw: s.yaw,
    sats: s.gps_sats ?? null,
    wpSeq: s.wp_seq ?? null,
  };
}

function near(a: number | null, b: number | null): boolean {
  if (a == null || b == null) return a === b;
  return Math.abs(a - b) < 1e-6;
}

function poseEq(a: Pose, b: Pose): boolean {
  return (
    a.ok === b.ok &&
    a.mode === b.mode &&
    a.sats === b.sats &&
    a.wpSeq === b.wpSeq &&
    near(a.lat, b.lat) &&
    near(a.lon, b.lon) &&
    near(a.gpsLat, b.gpsLat) &&
    near(a.gpsLon, b.gpsLon) &&
    near(a.truthLat, b.truthLat) &&
    near(a.truthLon, b.truthLon) &&
    near(a.homeLat, b.homeLat) &&
    near(a.homeLon, b.homeLon) &&
    near(a.alt, b.alt) &&
    near(a.gspd, b.gspd) &&
    near(a.hdg, b.hdg) &&
    Math.abs(a.yaw - b.yaw) < 2
  );
}

function originOf(pose: Pose): LatLon | null {
  return pair(pose.homeLat, pose.homeLon) ?? pair(pose.lat, pose.lon);
}

function storeKey(vehicle: string): string {
  return "arduloops.mission.v1." + vehicle;
}


function altMax(vehicle: string): number {
  return vehicle === "plane" ? 400 : 120;
}

function clampAlt(n: number, vehicle: string): number {
  return Math.min(altMax(vehicle), Math.max(2, Math.round(n)));
}

function loadPlan(vehicle: "copter" | "plane"): Plan {
  const alt = vehicle === "plane" ? 60 : 20;
  try {
    const raw = JSON.parse(localStorage.getItem(storeKey(vehicle)) || "") as Partial<Plan>;
    const points = Array.isArray(raw.points)
      ? (raw.points as unknown[]).map((pt) => cleanPoint(pt, vehicle)).filter((pt): pt is Pt => pt != null).slice(0, MAX_POINTS)
      : [];
    const nextAlt = Number(raw.alt);
    return {
      points,
      alt: Number.isFinite(nextAlt) && nextAlt > 0 ? clampAlt(nextAlt, vehicle) : alt,
      finish: raw.finish === "loiter" ? "loiter" : "rtl",
    };
  } catch {
    return { points: [], alt, finish: "rtl" };
  }
}

// Longitude goes the short way round, so a plan near ±180° stays in one piece.
function project(lat: number, lon: number, view: View, w: number, h: number): { x: number; y: number } {
  return projectRaw(lat, view.lon + wrap180(lon - view.lon), view, w, h);
}

// No wrap: tile edges run continuously past ±180° when the whole world is in view.
function projectRaw(lat: number, lon: number, view: View, w: number, h: number): { x: number; y: number } {
  const d = toEnu(lat, lon, view);
  return { x: w / 2 + d.e / view.mpp, y: h / 2 - d.n / view.mpp };
}

function unproject(x: number, y: number, view: View, w: number, h: number): LatLon {
  return fromEnu((x - w / 2) * view.mpp, (h / 2 - y) * view.mpp, view);
}

// New center so the lat/lon under (x, y) stays under that pixel. cos uses the
// new center's latitude, matching toEnu.
function zoomAbout(view: View, x: number, y: number, w: number, h: number, nextMpp: number): View {
  const under = unproject(x, y, view, w, h);
  const north = (h / 2 - y) * nextMpp;
  const east = (x - w / 2) * nextMpp;
  const lat = under.lat - north / M_PER_DEG;
  const cos = Math.cos((lat * Math.PI) / 180) || 1e-6;
  const lon = under.lon - east / (M_PER_DEG * cos);
  return { lat, lon, mpp: nextMpp };
}

// Base map layers. Tiles are Web Mercator XYZ; each tile's corners go through
// project(), and toEnu is separable in lat/lon, so tiles land as exact
// axis-aligned rects in our local frame with no gaps between neighbours.
type Layer = "grid" | "osm" | "sat" | "topo";
const LAYER_KEY = "arduloops.mission.layer";
const LAYERS: Record<Exclude<Layer, "grid">, { url: (z: number, x: number, y: number) => string; maxZoom: number; credit: string }> = {
  osm: {
    url: (z, x, y) => `https://tile.openstreetmap.org/${z}/${x}/${y}.png`,
    maxZoom: 19,
    credit: "© OpenStreetMap contributors",
  },
  sat: {
    url: (z, x, y) => `https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/${z}/${y}/${x}`,
    maxZoom: 19,
    credit: "Imagery © Esri, Maxar, Earthstar Geographics",
  },
  topo: {
    url: (z, x, y) => `https://tile.opentopomap.org/${z}/${x}/${y}.png`,
    maxZoom: 17,
    credit: "© OpenStreetMap contributors, SRTM · © OpenTopoMap (CC-BY-SA)",
  },
};
const TILE_CAP = 160;

function loadLayer(): Layer {
  try {
    const raw = localStorage.getItem(LAYER_KEY);
    if (raw === "osm" || raw === "sat" || raw === "topo") return raw;
  } catch {
    /* ignore */
  }
  return "grid";
}

function tileX(lon: number, n: number): number {
  return ((lon + 180) / 360) * n;
}

function tileY(lat: number, n: number): number {
  const r = (Math.max(-85.0511, Math.min(85.0511, lat)) * Math.PI) / 180;
  return ((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * n;
}

function tileLon(x: number, n: number): number {
  return (x / n) * 360 - 180;
}

function tileLat(y: number, n: number): number {
  return (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / n))) * 180) / Math.PI;
}

function Tiles({ layer, view, w, h }: { layer: Exclude<Layer, "grid">; view: View; w: number; h: number }) {
  const src = LAYERS[layer];
  const cos = Math.cos((view.lat * Math.PI) / 180) || 1e-6;
  // Pick the zoom whose native m/px is at or just below ours, so tiles are never upscaled
  // (until the layer's max zoom); back off if the viewport would need too many tiles.
  let z = Math.max(0, Math.min(src.maxZoom, Math.ceil(Math.log2((156_543.03392 * cos) / view.mpp))));
  const nw = unproject(0, 0, view, w, h);
  const se = unproject(w, h, view, w, h);
  let x0 = 0, x1 = 0, y0 = 0, y1 = 0, n = 1;
  for (;;) {
    n = 2 ** z;
    x0 = Math.floor(tileX(nw.lon, n));
    x1 = Math.floor(tileX(se.lon, n));
    y0 = Math.max(0, Math.floor(tileY(nw.lat, n)));
    y1 = Math.min(n - 1, Math.floor(tileY(se.lat, n)));
    if (z === 0 || (x1 - x0 + 1) * (y1 - y0 + 1) <= TILE_CAP) break;
    z -= 1;
  }
  const out: ReactElement[] = [];
  for (let ty = y0; ty <= y1; ty++) {
    const top = projectRaw(tileLat(ty, n), view.lon, view, w, h).y;
    const bottom = projectRaw(tileLat(ty + 1, n), view.lon, view, w, h).y;
    for (let tx = x0; tx <= x1; tx++) {
      const left = projectRaw(view.lat, tileLon(tx, n), view, w, h).x;
      const right = projectRaw(view.lat, tileLon(tx + 1, n), view, w, h).x;
      const wx = ((tx % n) + n) % n;
      // +0.5 px overlap hides antialiasing seams between neighbours.
      out.push(
        <image
          key={`${z}/${tx}/${ty}`}
          href={src.url(z, wx, ty)}
          x={left}
          y={top}
          width={right - left + 0.5}
          height={bottom - top + 0.5}
          preserveAspectRatio="none"
        />,
      );
    }
  }
  return <g className={"mission-tiles " + layer} pointerEvents="none">{out}</g>;
}

// Closest zoom is 0.15 m/px. Farthest shows the whole world across the map width.
// Waypoints are only added up to EDIT_MPP so a stray click cannot drop one a continent away.
const MIN_MPP = 0.15;
const EDIT_MPP = 50;
const EARTH_M = 40_075_016;

// Our frame scales longitude by cos(centre lat), so 360° of longitude spans EARTH_M * cos metres.
function maxMpp(w: number, lat: number): number {
  const cos = Math.max(0.2, Math.cos((lat * Math.PI) / 180));
  return Math.max(EDIT_MPP, (EARTH_M * cos) / Math.max(w, 1));
}

function wrapLon(lon: number): number {
  return ((((lon + 180) % 360) + 360) % 360) - 180;
}

// Keep the map edge from passing ±85° (the tile limit) and the zoom within maxMpp.
// Zoomed out, this pulls the centre toward the equator, where cos is near 1 and
// the world fits once across the width instead of repeating.
function settle(v: View, w: number, h: number): View {
  let mpp = Math.min(v.mpp, maxMpp(w, 0));
  const half = ((h / 2) * mpp) / M_PER_DEG;
  const lim = Math.max(0, 85 - half);
  const lat = Math.max(-lim, Math.min(lim, v.lat));
  mpp = Math.min(mpp, maxMpp(w, lat));
  return { lat, lon: wrapLon(v.lon), mpp };
}

function onEarth(p: LatLon): LatLon | null {
  const q = { lat: p.lat, lon: wrapLon(p.lon) };
  return isLat(q) && Math.abs(q.lat) <= 85 ? q : null;
}

function wheelSteps(ev: WheelEvent, height: number): number {
  let dy = ev.deltaY;
  if (ev.deltaMode === 1) dy *= 100;
  else if (ev.deltaMode === 2) dy *= height;
  if (!Number.isFinite(dy) || dy === 0) return 0;
  return Math.max(-6, Math.min(6, dy / 100));
}

function gridStep(mpp: number): number {
  const target = mpp * 90;
  const pow = 10 ** Math.floor(Math.log10(Math.max(target, 1e-3)));
  const n = target / pow;
  const m = n < 2 ? 1 : n < 5 ? 2 : 5;
  return m * pow;
}

function readCells(value: number | null, int: number, frac: number, signed: boolean, zero: boolean): string[] {
  const cells: string[] = [];
  const blank = value == null || !Number.isFinite(value);
  const neg = !blank && value < 0;
  if (signed) cells.push(blank || !neg ? "\u2007" : "-");
  if (blank) {
    for (let i = 0; i < int; i++) cells.push("\u2007");
    if (frac > 0) {
      cells.push("\u2007");
      for (let i = 0; i < frac; i++) cells.push("\u2007");
    }
    return cells;
  }
  const [whole, fraction] = Math.abs(value).toFixed(frac).split(".");
  const digits = whole.length > int ? whole : zero ? whole.padStart(int, "0") : whole.padStart(int, "\u2007");
  for (const ch of digits) cells.push(ch);
  if (frac > 0) {
    cells.push(".");
    for (const ch of fraction) cells.push(ch);
  }
  return cells;
}

function ReadNum({
  value,
  int,
  frac = 0,
  signed = false,
  zero = false,
}: {
  value: number | null;
  int: number;
  frac?: number;
  signed?: boolean;
  zero?: boolean;
}) {
  const cells = readCells(value, int, frac, signed, zero);
  const label = value == null || !Number.isFinite(value) ? undefined : (value < 0 ? "-" : "") + Math.abs(value).toFixed(frac);
  return (
    <span className="mission-num" aria-label={label}>
      {cells.map((ch, i) => (
        <span key={i} className="mission-cell" aria-hidden="true">
          {ch}
        </span>
      ))}
    </span>
  );
}

export function MissionView({
  current,
  ids,
  onWorkspace,
}: {
  current: WorkspaceId;
  ids: WorkspaceId[];
  onWorkspace: (id: WorkspaceId) => void;
}) {
  const t = useT();
  const vehicle = useVehicle();
  const pose = usePicked(pickPose, poseEq);
  const restart = usePicked((s) => s.params.MIS_RESTART);
  const tune = usePicked(pickTune, tuneEq);
  const [plan, setPlan] = useState<Plan>(() => loadPlan(vehicle));
  // The field keeps what is typed; the plan only takes a value inside the limits.
  const [altText, setAltText] = useState(() => String(plan.alt));
  const [view, setView] = useState<View | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [noteBad, setNoteBad] = useState(false);
  const [showL1, setShowL1] = useState(() => {
    try {
      return localStorage.getItem("arduloops.mission.l1") === "1";
    } catch {
      return false;
    }
  });
  const [layer, setLayer] = useState<Layer>(loadLayer);
  const [sel, setSel] = useState<number | null>(null);
  // The mission last read from the vehicle; the plan is "on board" while it matches.
  const [boardMission, setBoardMission] = useState<BoardItem[] | null>(null);
  const [, setTick] = useState(0);
  const mapRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const viewRef = useRef(view);
  const sizeRef = useRef(size);
  const opened = useRef(false);
  const drag = useRef<{ x: number; y: number; moved: boolean; point: number | null } | null>(null);
  // Whether ArduPlane flies the current AUTO leg without cross-track.
  const legStart = useRef<{ mode: string | null; seq: number | null; at: number; direct: boolean; prevTarget: LatLon | null }>({
    mode: null,
    seq: null,
    at: 0,
    direct: false,
    prevTarget: null,
  });
  const builtRef = useRef<{ built: Built | null; plan: Plan; synced: boolean }>({ built: null, plan, synced: false });
  const onWheelRef = useRef<(ev: WheelEvent) => void>(() => {});
  viewRef.current = view;
  sizeRef.current = size;

  onWheelRef.current = (ev: WheelEvent) => {
    const current = viewRef.current;
    const box = sizeRef.current;
    const svg = svgRef.current;
    if (!current || !svg || box.w < 40 || box.h < 40) return;
    const rect = svg.getBoundingClientRect();
    if (rect.width < 1 || rect.height < 1) return;
    ev.preventDefault();
    const steps = wheelSteps(ev, box.h);
    if (!steps) return;
    const nextMpp = Math.min(maxMpp(box.w, current.lat), Math.max(MIN_MPP, current.mpp * Math.exp(steps * Math.log(1.12))));
    if (nextMpp === current.mpp) return;
    const x = ((ev.clientX - rect.left) / rect.width) * box.w;
    const y = ((ev.clientY - rect.top) / rect.height) * box.h;
    const next = settle(zoomAbout(current, x, y, box.w, box.h, nextMpp), box.w, box.h);
    viewRef.current = next;
    setView(next);
  };

  useEffect(() => {
    if (!pose.ok || restart != null) return;
    send({ op: "param_read", name: "MIS_RESTART" });
  }, [pose.ok, restart]);

  const leg = vehicle === "plane" ? 250 : 80;
  const side = vehicle === "plane" ? 1000 : 300;
  const origin = originOf(pose);
  const board = pair(pose.lat, pose.lon);
  const gps = pair(pose.gpsLat, pose.gpsLon);
  const truth = pair(pose.truthLat, pose.truthLon);
  const home = pair(pose.homeLat, pose.homeLon);

  // The assistant sees this plan while the Mission screen is open.
  const originKey = origin ? `${origin.lat.toFixed(6)},${origin.lon.toFixed(6)}` : "";
  useEffect(() => {
    setScreenView({ view: "mission", vehicle, detail: missionDetail(plan, origin, vehicle, pose, getSnapshot().params) });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [plan, originKey, vehicle, pose.mode, pose.wpSeq, tune]);
  useEffect(() => () => setScreenView(null), []);

  // Mission progress comes from ArduPilot's "Mission: <seq> <name>" reports.
  useEffect(() => {
    hookMissionRun();
    return onMissionItem((seq) => {
      const { built: b, plan: pl, synced: on } = builtRef.current;
      const role = on ? b?.roles[seq] : undefined;
      if (role?.role !== "act") return;
      const act = pl.points[role.point]?.acts?.[role.act];
      if (act) addLog(t("Point {n}: {what}", { n: role.point + 1, what: actText(act, t) }), "ok");
      setTick((n) => n + 1);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // Read the board's mission once per link, so the screen knows whether the plan is on board.
  const autoRead = useRef(false);
  useEffect(() => {
    if (!pose.ok) {
      autoRead.current = false;
      return;
    }
    if (autoRead.current) return;
    autoRead.current = true;
    const id = window.setTimeout(() => {
      void readMission().then((r) => {
        if ("items" in r) setBoardMission(r.items);
      });
    }, 1500);
    return () => window.clearTimeout(id);
  }, [pose.ok]);
  useEffect(() => {
    if (pose.mode !== "AUTO") return;
    const id = window.setInterval(() => setTick((n) => n + 1), 1000);
    return () => window.clearInterval(id);
  }, [pose.mode]);

  useEffect(() => {
    try {
      localStorage.setItem(storeKey(vehicle), JSON.stringify(plan));
    } catch {
      /* ignore */
    }
  }, [plan, vehicle]);

  useEffect(() => {
    try {
      localStorage.setItem("arduloops.mission.l1", showL1 ? "1" : "0");
      localStorage.setItem(LAYER_KEY, layer);
    } catch {
      /* ignore */
    }
  }, [showL1, layer]);

  useEffect(() => {
    const el = mapRef.current;
    if (!el) return;
    const sync = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  function fitTo(pts: LatLon[]) {
    const kept = pts.filter(isLat);
    if (!kept.length) return;
    const { w, h } = sizeRef.current;
    // Average longitude as offsets from the first point, so points either side of ±180° centre between them.
    const lon0 = kept[0].lon;
    let lat = 0;
    let dLon = 0;
    for (const p of kept) {
      lat += p.lat;
      dLon += wrap180(p.lon - lon0);
    }
    lat /= kept.length;
    const lon = wrapLon(lon0 + dLon / kept.length);
    const c = { lat, lon };
    let max = 25;
    for (const p of kept) {
      const d = toEnu(p.lat, lon + wrap180(p.lon - lon), c);
      max = Math.max(max, Math.abs(d.e), Math.abs(d.n));
    }
    const span = Math.max(80, Math.min(w, h));
    const mpp = (max * 2.6) / span;
    setView(settle({ lat, lon, mpp: Math.min(EDIT_MPP, Math.max(MIN_MPP, mpp)) }, w, h));
  }

  useEffect(() => {
    if (opened.current || size.w < 40) return;
    const pts = [origin, ...plan.points, board].filter(isLat);
    if (!pts.length) return;
    opened.current = true;
    fitTo(pts);
  }, [size.w, size.h, pose.lat, pose.homeLat, plan.points.length]);

  useEffect(() => {
    const el = mapRef.current;
    if (!el) return;
    const onWheel = (ev: WheelEvent) => onWheelRef.current(ev);
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  function localXY(ev: { clientX: number; clientY: number }): { x: number; y: number } | null {
    const el = svgRef.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: ev.clientX - r.left, y: ev.clientY - r.top };
  }

  function hitPoint(x: number, y: number): number | null {
    const current = viewRef.current;
    const box = sizeRef.current;
    if (!current) return null;
    let best: number | null = null;
    let bestD = 14;
    plan.points.forEach((p, i) => {
      const at = project(p.lat, p.lon, current, box.w, box.h);
      const d = Math.hypot(at.x - x, at.y - y);
      if (d < bestD) {
        best = i;
        bestD = d;
      }
    });
    return best;
  }

  function lockSelect(on: boolean) {
    document.body.classList.toggle("mission-drag", on);
    if (on) window.getSelection()?.removeAllRanges();
  }

  function onPointerDown(ev: ReactPointerEvent<SVGSVGElement>) {
    if (!viewRef.current) return;
    const at = localXY(ev);
    if (!at) return;
    ev.preventDefault();
    drag.current = { x: ev.clientX, y: ev.clientY, moved: false, point: hitPoint(at.x, at.y) };
    lockSelect(true);
    ev.currentTarget.setPointerCapture(ev.pointerId);
  }

  function onPointerMove(ev: ReactPointerEvent<SVGSVGElement>) {
    const d = drag.current;
    const current = viewRef.current;
    const box = sizeRef.current;
    if (!d || !current || box.w < 40) return;
    const dx = ev.clientX - d.x;
    const dy = ev.clientY - d.y;
    if (!d.moved && Math.hypot(dx, dy) < 4) return;
    d.moved = true;
    d.x = ev.clientX;
    d.y = ev.clientY;
    if (d.point != null) {
      const at = localXY(ev);
      if (!at) return;
      const next = onEarth(unproject(at.x, at.y, current, box.w, box.h));
      if (!next) return;
      const idx = d.point;
      setPlan((p) => ({ ...p, points: p.points.map((pt, i) => (i === idx ? next : pt)) }));
      return;
    }
    const shifted = fromEnu(-dx * current.mpp, dy * current.mpp, current);
    setView(settle({ ...current, lat: shifted.lat, lon: shifted.lon }, box.w, box.h));
  }

  function onPointerUp(ev: ReactPointerEvent<SVGSVGElement>) {
    lockSelect(false);
    const d = drag.current;
    drag.current = null;
    const current = viewRef.current;
    const box = sizeRef.current;
    if (d && !d.moved && d.point != null) {
      setSel((s) => (s === d.point ? null : d.point));
      return;
    }
    if (!d || d.moved || d.point != null || !current || box.w < 40 || current.mpp > EDIT_MPP) return;
    const at = localXY(ev);
    if (!at) return;
    const next = onEarth(unproject(at.x, at.y, current, box.w, box.h));
    if (!next) return;
    setPlan((p) => (p.points.length >= MAX_POINTS ? p : { ...p, points: [...p.points, next] }));
  }

  function onNorth() {
    if (!origin) return;
    const points = [fromEnu(0, leg, origin)];
    setPlan((p) => ({ ...p, points, finish: "rtl" }));
    fitTo([origin, ...points]);
  }

  function onSquare() {
    if (!origin) return;
    const points = [fromEnu(0, side, origin), fromEnu(side, side, origin), fromEnu(side, 0, origin)];
    setPlan((p) => ({ ...p, points, finish: "rtl" }));
    fitTo([origin, ...points]);
  }

  function onClover() {
    if (!origin) return;
    const points: LatLon[] = [];
    const n = 32;
    const near = side * 0.12;
    for (let i = 0; i < n; i++) {
      const th = (i / n) * Math.PI * 2;
      const radius = side * Math.cos(2 * th);
      const east = radius * Math.sin(th);
      const north = radius * Math.cos(th);
      if (east * east + north * north < near * near) continue;
      points.push(fromEnu(east, north, origin));
    }
    setPlan((p) => ({ ...p, points, finish: "rtl" }));
    fitTo([origin, ...points]);
  }

  function onLoiter() {
    if (!origin) return;
    setPlan((p) => ({ ...p, points: [], finish: "loiter" }));
    fitTo([origin, board].filter(isLat));
  }

  function say(message: string, bad: boolean) {
    setNote(message);
    setNoteBad(bad);
    addLog(KNOWN.has(message) ? t(message) : message, bad ? "bad" : "ok");
  }

  async function readBack(): Promise<BoardItem[] | null> {
    const r = await readMission();
    if ("error" in r) {
      say(r.error, true);
      return null;
    }
    setBoardMission(r.items);
    return r.items;
  }

  async function onUpload() {
    if (!origin || busy) return;
    setBusy(true);
    const b = buildMission(origin, { ...plan, alt: clampAlt(plan.alt, vehicle) }, vehicle);
    const err = await uploadMission(b.items);
    if (!err) {
      resetMissionRun();
      // Read it back: the plan counts as on board only once the vehicle returns the same items.
      const got = await readBack();
      setBusy(false);
      if (got && !sameMission(b.items, got)) say(t("The board returned a different mission."), true);
      else say("Mission uploaded", false);
      return;
    }
    setBusy(false);
    say(err, true);
  }

  async function onErase() {
    if (!pose.ok || busy) return;
    setBusy(true);
    const err = await uploadMission([]);
    if (!err) await readBack();
    setBusy(false);
    say(err ?? "Mission cleared", err != null);
  }

  async function onRead() {
    if (!pose.ok || busy) return;
    setBusy(true);
    await readBack();
    setBusy(false);
  }

  function onLoadBoard() {
    if (!boardMission) return;
    const { plan: next, skipped } = boardToPlan(boardMission, vehicle, plan.alt);
    setPlan({ ...next, alt: clampAlt(next.alt, vehicle) });
    setAltText(String(clampAlt(next.alt, vehicle)));
    setSel(null);
    if (skipped) say(t("{n} items this screen does not edit were left out.", { n: skipped }), true);
  }

  function onRestart(on: boolean) {
    const value = on ? 1 : 0;
    send({ op: "param", name: "MIS_RESTART", value });
    noteUi([{ kind: "param", name: "MIS_RESTART", value, from: restart ?? null }]);
    addLog(t(on ? "Restart in AUTO" : "Resume in AUTO"), "cmd");
  }

  function onMode(mode: "AUTO" | "RTL") {
    send({ op: "mode", mode });
    noteUi([{ kind: "mode", from: pose.mode, mode }]);
    addLog(t("Mode {mode}", { mode }), "cmd");
  }

  const truthM = board && truth ? dist(board, truth) : null;
  const gpsM = board && gps ? dist(board, gps) : null;
  const offM = truthM ?? gpsM;
  const offHot = (offM ?? 0) > 2;
  const hdg = pose.hdg == null ? null : ((Math.round(pose.hdg) % 360) + 360) % 360;
  const noteText = note ? (KNOWN.has(note) ? t(note) : note) : null;
  const route = [origin, ...plan.points, endsHome(plan) ? origin : null].filter(isLat);
  const built = origin ? buildMission(origin, { ...plan, alt: clampAlt(plan.alt, vehicle) }, vehicle) : null;
  const synced = !!(built && boardMission && sameMission(built.items, boardMission));
  builtRef.current = { built, plan, synced };
  const livePoint = (() => {
    if (!synced || !built || pose.mode !== "AUTO" || pose.wpSeq == null) return null;
    const role = built.roles[pose.wpSeq];
    return role?.role === "point" && role.point >= 0 ? role.point : null;
  })();
  const wpDist = getSnapshot().wp_dist ?? null;
  function actState(i: number, j: number): ActState {
    if (!synced || !built) return null;
    const ps = startedAt(built.pointSeq[i]);
    if (ps == null) return { state: "pending" };
    const seq = built.pointSeq[i] + 1 + j;
    const s = startedAt(seq);
    const acts = plan.points[i]?.acts ?? [];
    const act = acts[j];
    if (!act || s == null || s < ps) return { state: "pending" };
    // A condition is met once the next held action starts (a jump is not held).
    const heldK = acts.findIndex((x, k) => k > j && x.kind !== "jump");
    if (ACTS[act.kind].cond && heldK > j) {
      const nextS = startedAt(seq + (heldK - j));
      if (nextS == null || nextS < s) {
        const left =
          act.kind === "delay"
            ? t("{v} s", { v: Math.max(0, Math.ceil(act.a - (Date.now() - s) / 1000)) })
            : act.kind === "distance" && wpDist != null
              ? t("{v} m", { v: Math.max(0, Math.round(wpDist - act.a)) })
              : act.kind === "yaw" && pose.hdg != null
                ? t("now {v}°", { v: ((Math.round(pose.hdg) % 360) + 360) % 360 })
                : undefined;
        return { state: "wait", left };
      }
    }
    return { state: "done" };
  }
  // Mode::enter and verify_takeoff clear next_wp_crosstrack: the leg after AUTO entry
  // or after the takeoff is flown direct. MISSION_CURRENT can trail HEARTBEAT, so a
  // sequence change right after entry still counts as the entry leg. A page opened
  // mid-AUTO cannot tell and assumes cross-track.
  {
    const ls = legStart.current;
    const now = Date.now();
    if (ls.mode == null) {
      ls.mode = pose.mode;
      ls.seq = pose.wpSeq;
    } else if (pose.mode !== "AUTO") {
      ls.direct = false;
      ls.prevTarget = null;
    } else if (ls.mode !== "AUTO") {
      ls.direct = true;
      ls.prevTarget = null;
      ls.at = now;
    } else if (pose.wpSeq !== ls.seq) {
      ls.direct = ls.seq === 1 || now - ls.at < 1500;
      const was = ls.seq != null ? built?.roles[ls.seq] : undefined;
      ls.prevTarget = was?.role === "point" ? (was.point < 0 ? origin : (plan.points[was.point] ?? null)) : null;
    }
    ls.mode = pose.mode;
    ls.seq = pose.wpSeq;
  }
  const nav =
    vehicle === "plane" && showL1 && board
      ? chooseNav(
          pose.mode,
          pose.wpSeq,
          board,
          (pose.gspd ?? 0) >= 1,
          home,
          origin,
          plan,
          built?.roles ?? [],
          legStart.current.direct,
          legStart.current.prevTarget,
          tune,
        )
      : null;
  const e2t = eas2tasAt((getSnapshot().home_alt_m ?? 0) + (pose.alt ?? 0));
  const l1 =
    board && nav
      ? nav.kind === "line"
        ? solveL1(board, nav.a, nav.b, pose.yaw, pose.hdg, pose.gspd, tune, nav.turnDeg, e2t)
        : nav.kind === "loiter"
          ? solveLoiter(board, nav.center, nav.r, nav.dir, pose.yaw, pose.hdg, pose.gspd, tune, e2t)
          : null
      : null;

  return (
    <section className="scope work mission">
      <div className="work-bar">
        <WorkspaceTabs current={current} ids={ids} onSelect={onWorkspace} />
        <div className="mission-acts">
          <button type="button" className="cyan" disabled={!origin || busy} onClick={() => void onUpload()}>
            {busy ? t("Uploading…") : t("Upload mission")}
          </button>
          <button type="button" disabled={!pose.ok || busy} onClick={() => onMode("AUTO")} title={t("Start the mission")}>
            AUTO
          </button>
          <button type="button" disabled={!pose.ok || busy} onClick={() => onMode("RTL")} title={t("Return home")}>
            RTL
          </button>
        </div>
      </div>
      <div className="mission-body">
        <div className={"mission-map" + (layer === "grid" ? "" : " tiled")} ref={mapRef}>
          <svg
            ref={svgRef}
            width={Math.max(size.w, 1)}
            height={Math.max(size.h, 1)}
            viewBox={`0 0 ${Math.max(size.w, 1)} ${Math.max(size.h, 1)}`}
            role="application"
            aria-label={t("mission")}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onPointerCancel={() => { drag.current = null; lockSelect(false); }}
            onLostPointerCapture={() => lockSelect(false)}
          >
            {view && size.w > 40 && layer !== "grid" ? <Tiles layer={layer} view={view} w={size.w} h={size.h} /> : null}
            {view && size.w > 40 ? (
              <MapArt
                marks={
                  origin ? (
                    <MissionMarks
                      plan={plan}
                      origin={origin}
                      vehicle={vehicle}
                      tune={tune}
                      e2t={e2t}
                      sel={sel}
                      actState={actState}
                      xy={(p) => project(p.lat, p.lon, view, size.w, size.h)}
                      mpp={view.mpp}
                    />
                  ) : null
                }
                tiled={layer !== "grid"}
                view={view}
                w={size.w}
                h={size.h}
                route={route}
                points={plan.points}
                finish={plan.finish}
                home={home}
                board={board}
                gps={gps}
                truth={truth}
                yaw={pose.yaw}
                livePoint={livePoint}
                homeHot={(() => {
                  if (pose.mode !== "AUTO" || pose.wpSeq == null || !built) return false;
                  const role = built.roles[pose.wpSeq];
                  return role?.role === "home" || role?.role === "rtl" || (role?.role === "point" && role.point < 0);
                })()}
                l1={l1}
              />
            ) : null}
          </svg>
          <div className="mission-tools">
            <select
              className="mission-layer"
              value={layer}
              onChange={(ev) => setLayer(ev.target.value as Layer)}
              aria-label={t("Map layer")}
              title={t("Map layer")}
            >
              <option value="grid">{t("Grid")}</option>
              <option value="osm">{t("Street map")}</option>
              <option value="sat">{t("Satellite")}</option>
              <option value="topo">{t("Topographic")}</option>
            </select>
            <button type="button" className="mission-fit" onClick={() => fitTo([origin, board, truth, gps, ...plan.points].filter(isLat))}>
              {t("Fit map")}
            </button>
          </div>
          {layer !== "grid" ? <span className="mission-credit">{LAYERS[layer].credit}</span> : null}
          <div className="mission-read">
            <span className="mission-mode">{pose.mode}</span>
            <span>
              <ReadNum value={pose.alt} int={3} frac={1} signed />
              {t("m")}
            </span>
            <span>
              <ReadNum value={pose.gspd} int={2} frac={1} signed />
              {t("m/s")}
            </span>
            <span>
              <ReadNum value={hdg} int={3} zero />°
            </span>
            <span>
              <ReadNum value={pose.sats} int={2} />
              {t("sats")}
            </span>
            <span>
              <span className="mission-kind">
                <span className="ghost">{t("item")}</span>
                <span>{pose.mode === "AUTO" ? t("item") : "\u00a0"}</span>
              </span>
              <ReadNum value={pose.mode === "AUTO" ? pose.wpSeq : null} int={2} />
            </span>
            <span className={offHot ? "off" : undefined}>
              <span className="mission-kind">
                <span className="ghost">{t("off truth")}</span>
                <span className="ghost">{t("off GPS")}</span>
                <span>{truthM != null ? t("off truth") : gpsM != null ? t("off GPS") : "\u00a0"}</span>
              </span>
              <ReadNum value={offM} int={4} frac={1} />
              {t("m")}
            </span>
          </div>
          {!view ? <p className="mission-empty">{t("No position yet.")}</p> : null}
        </div>
        <aside className="mission-side">
          <label>
            {t("Altitude, m")}
            <input
              type="number"
              min={2}
              max={altMax(vehicle)}
              step={1}
              value={altText}
              onChange={(ev) => {
                const text = ev.target.value;
                setAltText(text);
                const n = Number(text);
                if (text.trim() !== "" && Number.isFinite(n) && n >= 2 && n <= altMax(vehicle)) {
                  setPlan((p) => ({ ...p, alt: Math.round(n) }));
                }
              }}
              onBlur={() => setAltText(String(plan.alt))}
              onKeyDown={(ev) => {
                if (ev.key === "Enter") ev.currentTarget.blur();
              }}
            />
          </label>
          <div className="mission-presets">
            <button type="button" disabled={!origin} onClick={onLoiter} title={t("Loiter here")}>
              {t("Loiter here")}
            </button>
            <button type="button" disabled={!origin} onClick={onNorth} title={t("{v} m", { v: leg })}>
              {t("North leg")}
            </button>
            <button type="button" disabled={!origin} onClick={onSquare} title={t("{v} m", { v: side })}>
              {t("Square")}
            </button>
            <button type="button" disabled={!origin} onClick={onClover} title={t("{v} m", { v: side })}>
              {t("Clover")}
            </button>
          </div>
          <label className="mission-rtl">
            <input
              type="checkbox"
              checked={plan.finish === "rtl"}
              onChange={(ev) => setPlan((p) => ({ ...p, finish: ev.target.checked ? "rtl" : "loiter" }))}
            />
            {t("End with RTL")}
          </label>
          <label className="mission-rtl" title={t("Entering AUTO starts at the first waypoint. Clear it and AUTO continues where it left off.")}>
            <input
              type="checkbox"
              checked={restart != null && restart >= 0.5}
              disabled={!pose.ok || restart == null}
              onChange={(ev) => onRestart(ev.target.checked)}
            />
            {t("Restart in AUTO")}
          </label>
          {vehicle === "plane" ? (
            <label className="mission-rtl" title={t("Circle of radius L1, the look-ahead point, Nu, and the cross-track error.")}>
              <input type="checkbox" checked={showL1} onChange={(ev) => setShowL1(ev.target.checked)} />
              {t("Show L1 track")}
            </label>
          ) : null}
          {nav?.kind === "takeoff" ? <p className="mission-hint">{t("Takeoff: course hold, no L1 track.")}</p> : null}
          {l1 ? (
            <p className="mission-hint mission-l1">
              {l1.preview ? `${t("cruise")} · ` : ""}
              {l1.phase === "line"
                ? t("L1 {n} m · Nu {nu}° · off {x} m", {
                    n: Math.round(l1.l1m),
                    nu: Math.round(l1.nuDeg),
                    x: Math.abs(l1.xtrack).toFixed(0),
                  })
                : l1.phase === "capture"
                  ? t("Loiter {r} m: to the centre · Nu {nu}°", { r: Math.round(l1.circle?.r ?? 0), nu: Math.round(l1.nuDeg) })
                  : t("Loiter {r} m: on the circle · off {x} m", {
                      r: Math.round(l1.circle?.r ?? 0),
                      x: l1.xtrack.toFixed(0),
                    })}
              {" · "}
              {t("bank {b}°", { b: Math.round(l1.rollDeg) })}
            </p>
          ) : null}
          <div className="mission-pts-head">
            <b>{t("Waypoints")}</b>
            <button type="button" disabled={!plan.points.length} onClick={() => setPlan((p) => ({ ...p, points: [] }))}>
              {t("Clear points")}
            </button>
          </div>
          <MissionEditor
            plan={plan}
            setPlan={setPlan}
            vehicle={vehicle}
            origin={origin}
            sel={sel}
            onSelect={setSel}
            livePoint={livePoint}
            actState={actState}
          />
          <p className="mission-hint">
            {plan.points.length >= MAX_POINTS
              ? t("{n} waypoints is the limit.", { n: MAX_POINTS })
              : view && view.mpp > EDIT_MPP
                ? t("Zoom in to add waypoints.")
                : t("Click the map to add a waypoint.")}
          </p>
          <ul className="mission-legend">
            <li><i className="swatch board" />{t("board")}</li>
            <li><i className="swatch gps" />{t("GPS fix")}</li>
            <li><i className="swatch truth" />{t("true position")}</li>
            <li><i className="swatch home" />{t("home")}</li>
            {vehicle === "plane" && showL1 ? (
              <>
                <li><i className="swatch l1" />{t("L1 track")}</li>
                <li><i className="swatch nu" />Nu</li>
                <li><i className="swatch wp" />{t("turn start")}</li>
              </>
            ) : null}
          </ul>
          <div className="mission-board">
            <button type="button" disabled={!pose.ok || busy} onClick={() => void onRead()}>
              {t("Read from board")}
            </button>
            <button type="button" disabled={!pose.ok || busy} onClick={() => void onErase()}>
              {t("Erase mission")}
            </button>
          </div>
          {boardMission ? (
            <p className={synced ? "mission-hint mission-sync on" : "mission-hint mission-sync off"}>
              {synced
                ? t("The board has this plan.")
                : boardMission.length <= 1
                  ? t("The board has no mission.")
                  : t("The board has a different mission: {n} items.", { n: boardMission.length })}
              {!synced && boardMission.length > 1 ? (
                <button type="button" onClick={onLoadBoard}>
                  {t("Load into plan")}
                </button>
              ) : null}
            </p>
          ) : null}
          {noteText ? <p className={noteBad ? "mission-msg bad" : "mission-msg"}>{noteText}</p> : null}
          <p className="mission-hint">
            {t("Arm from the side, then AUTO. Jam and glitch sit on the simulation rail: the board follows the GPS fix, and truth is where the vehicle actually is.")}
          </p>
        </aside>
      </div>
    </section>
  );
}

function MapArt({
  marks,
  tiled,
  view,
  w,
  h,
  route,
  points,
  finish,
  home,
  board,
  gps,
  truth,
  yaw,
  livePoint,
  homeHot,
  l1,
}: {
  marks: ReactElement | null;
  tiled: boolean;
  view: View;
  w: number;
  h: number;
  route: LatLon[];
  points: LatLon[];
  finish: Finish;
  home: LatLon | null;
  board: LatLon | null;
  gps: LatLon | null;
  truth: LatLon | null;
  yaw: number;
  livePoint: number | null;
  homeHot: boolean;
  l1: L1Picture | null;
}) {
  const step = gridStep(view.mpp);
  const anchor = home ?? board ?? view;
  const o = toEnu(anchor.lat, anchor.lon, view);
  const halfE = (w / 2) * view.mpp;
  const halfN = (h / 2) * view.mpp;
  const lines: { x1: number; y1: number; x2: number; y2: number }[] = [];
  const e0 = Math.floor((-halfE - o.e) / step) * step;
  const e1 = halfE - o.e;
  for (let e = e0; e <= e1 && lines.length < 80; e += step) {
    const x = w / 2 + (o.e + e) / view.mpp;
    lines.push({ x1: x, y1: 0, x2: x, y2: h });
  }
  const n0 = Math.floor((-halfN - o.n) / step) * step;
  const n1 = halfN - o.n;
  for (let n = n0; n <= n1 && lines.length < 80; n += step) {
    const y = h / 2 - (o.n + n) / view.mpp;
    lines.push({ x1: 0, y1: y, x2: w, y2: y });
  }
  const xy = (p: LatLon) => project(p.lat, p.lon, view, w, h);
  const routePts = route.map(xy).map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" ");
  const boardTrail = trail.map((p) => xy(p)).map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" ");
  const truthTrail = trail
    .filter((p) => p.truth)
    .map((p) => xy(p.truth as LatLon))
    .map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`)
    .join(" ");
  const bar = Math.max(24, step / view.mpp);
  const boardAt = board ? xy(board) : null;
  const truthAt = truth ? xy(truth) : null;
  const gpsAt = gps ? xy(gps) : null;

  return (
    <>
      {lines.map((ln, i) => (
        <line key={i} x1={ln.x1} y1={ln.y1} x2={ln.x2} y2={ln.y2} stroke={tiled ? "#ffffff" : "#24303a"} strokeOpacity={tiled ? 0.18 : 1} strokeWidth="1" />
      ))}
      <text x={w / 2} y={tiled ? 40 : 16} textAnchor="middle" fill="#8b98a8" fontSize="11">N</text>
      {truthTrail ? <polyline points={truthTrail} fill="none" stroke="#81c784" strokeWidth="1.5" opacity="0.8" /> : null}
      {boardTrail ? <polyline points={boardTrail} fill="none" stroke="#4fc3f7" strokeWidth="1.5" opacity="0.55" /> : null}
      {routePts && tiled ? <polyline points={routePts} fill="none" stroke="#101214" strokeWidth="3.6" strokeOpacity="0.6" strokeLinejoin="round" /> : null}
      {routePts ? <polyline points={routePts} fill="none" stroke="#e8eef4" strokeWidth="1.4" strokeDasharray="5 4" /> : null}
      {boardAt && truthAt && dist(board as LatLon, truth as LatLon) > 1 ? (
        <line x1={boardAt.x} y1={boardAt.y} x2={truthAt.x} y2={truthAt.y} stroke="#ef5350" strokeWidth="1.2" strokeDasharray="3 3" />
      ) : null}
      {boardAt && gpsAt && dist(board as LatLon, gps as LatLon) > 1 ? (
        <line x1={boardAt.x} y1={boardAt.y} x2={gpsAt.x} y2={gpsAt.y} stroke="#ffb74d" strokeWidth="1" strokeDasharray="2 3" />
      ) : null}
      {finish === "loiter" && !points.length && route[0] ? (
        <circle cx={xy(route[0]).x} cy={xy(route[0]).y} r="13" fill="none" stroke="#ffb74d" strokeWidth="1.5" />
      ) : null}
      {marks}
      {home ? <HomeMark at={xy(home)} hot={homeHot} /> : null}
      {points.map((p, i) => {
        const at = xy(p);
        const hot = livePoint === i;
        const hold = finish === "loiter" && i === points.length - 1;
        return (
          <g key={i}>
            <circle cx={at.x} cy={at.y} r={hold ? 7 : 5} fill={hot ? "#4fc3f7" : "#e8eef4"} stroke={hold ? "#ffb74d" : "#101214"} strokeWidth="1.5" />
            <text x={at.x + 8} y={at.y - 6} fill="#8b98a8" fontSize="11">{i + 1}</text>
          </g>
        );
      })}
      {gpsAt ? <circle cx={gpsAt.x} cy={gpsAt.y} r="4" fill="#ffb74d" /> : null}
      {truthAt ? <circle cx={truthAt.x} cy={truthAt.y} r="6" fill="none" stroke="#81c784" strokeWidth="2" /> : null}
      {l1 ? <L1Marks picture={l1} xy={xy} mpp={view.mpp} cue={false} /> : null}
      {boardAt ? (
        <g transform={`translate(${boardAt.x} ${boardAt.y}) rotate(${yaw})`}>
          <polygon points="0,-9 6,7 -6,7" fill="#4fc3f7" />
        </g>
      ) : null}
      {l1 ? <L1Marks picture={l1} xy={xy} mpp={view.mpp} cue /> : null}
      <line x1={w - 12 - bar} y1={h - 16} x2={w - 12} y2={h - 16} stroke="#e8eef4" strokeWidth="2" />
      <text x={w - 12 - bar} y={h - 20} fill="#8b98a8" fontSize="11">{step >= 1000 ? `${step / 1000} km` : `${step} m`}</text>
    </>
  );
}

function compassPt(at: { x: number; y: number }, deg: number, r: number): { x: number; y: number } {
  const rad = (deg * Math.PI) / 180;
  return { x: at.x + Math.sin(rad) * r, y: at.y - Math.cos(rad) * r };
}

function L1Marks({
  picture,
  xy,
  mpp,
  cue,
}: {
  picture: L1Picture;
  xy: (p: LatLon) => { x: number; y: number };
  mpp: number;
  cue: boolean;
}) {
  const at = xy(picture.at);
  const a = xy(picture.a);
  const b = xy(picture.b);
  const ref = xy(picture.ref);
  const foot = picture.foot ? xy(picture.foot) : null;
  const radius = picture.l1m / mpp;
  const arcR = Math.max(10, Math.min(28, radius * 0.42));
  const vel = Math.max(16, Math.min(46, radius * 0.55));
  const nose = compassPt(at, picture.courseDeg, 11);
  const tip = compassPt(at, picture.courseDeg, 11 + vel);
  const head = 5;
  const back = compassPt(tip, picture.courseDeg + 180, head * 2);
  const left = compassPt(back, picture.courseDeg + 90, head);
  const right = compassPt(back, picture.courseDeg - 90, head);
  const showArc = Math.abs(picture.nuDeg) >= 4 && arcR >= 8;
  const arc0 = compassPt(at, picture.courseDeg, arcR);
  const arc1 = compassPt(at, picture.courseDeg + picture.nuDeg, arcR);
  const mid = compassPt(at, picture.courseDeg + picture.nuDeg / 2, arcR + 11);
  const leadR = picture.leadM != null ? picture.leadM / mpp : 0;
  if (cue) {
    return (
      <g pointerEvents="none">
        <line x1={nose.x} y1={nose.y} x2={tip.x} y2={tip.y} stroke="#4fc3f7" strokeWidth="1.6" strokeLinecap="round" />
        <polygon points={`${tip.x},${tip.y} ${left.x},${left.y} ${right.x},${right.y}`} fill="#4fc3f7" />
        {showArc ? (
          <path
            d={`M ${arc0.x.toFixed(1)} ${arc0.y.toFixed(1)} A ${arcR.toFixed(1)} ${arcR.toFixed(1)} 0 0 ${picture.nuDeg >= 0 ? 1 : 0} ${arc1.x.toFixed(1)} ${arc1.y.toFixed(1)}`}
            fill="none"
            stroke="#8b98a8"
            strokeWidth="1.5"
          />
        ) : null}
        {showArc ? (
          <text x={mid.x} y={mid.y} fill="#8b98a8" fontSize="11" textAnchor="middle">
            Nu
          </text>
        ) : null}
      </g>
    );
  }
  return (
    <g pointerEvents="none">
      {leadR >= 6 ? (
        <circle cx={b.x} cy={b.y} r={leadR} fill="none" stroke="#8b98a8" strokeWidth="1" strokeDasharray="3 4" opacity="0.75" />
      ) : null}
      {picture.circle ? (
        <circle
          cx={a.x}
          cy={a.y}
          r={Math.max(2, picture.circle.r / mpp)}
          fill="none"
          stroke="#ffb74d"
          strokeWidth="2.4"
          opacity="0.9"
        />
      ) : (
        <line x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke="#ffb74d" strokeWidth="2.4" strokeLinecap="round" opacity="0.9" />
      )}
      {foot ? (
        <line x1={at.x} y1={at.y} x2={foot.x} y2={foot.y} stroke="#ffb74d" strokeWidth="1" strokeDasharray="3 3" />
      ) : null}
      {radius >= 3 && picture.phase !== "circle" ? (
        <circle cx={at.x} cy={at.y} r={radius} fill="none" stroke="#ffb74d" strokeWidth="1.3" strokeDasharray="5 4" />
      ) : null}
      <line x1={at.x} y1={at.y} x2={ref.x} y2={ref.y} stroke="#ffb74d" strokeWidth="1.6" />
      {picture.along ? <circle cx={ref.x} cy={ref.y} r="4.5" fill="#ffb74d" stroke="#101214" strokeWidth="1.2" /> : null}
    </g>
  );
}

// Point kinds, actions, jumps, ROI lines, distance gates and speed changes on the map.
function MissionMarks({
  plan,
  origin,
  vehicle,
  tune,
  e2t,
  sel,
  actState,
  xy,
  mpp,
}: {
  plan: Plan;
  origin: LatLon;
  vehicle: string;
  tune: L1Tune;
  e2t: number;
  sel: number | null;
  actState: (i: number, j: number) => ActState;
  xy: (p: LatLon) => { x: number; y: number };
  mpp: number;
}) {
  const t = useT();
  const { points } = plan;
  const loiterR = loiterRadius(loiterOf(0, tune).r, tune, e2t) / mpp;
  const speedLegs: { a: LatLon; b: LatLon; v: number }[] = [];
  let speed: number | null = null;
  points.forEach((p, i) => {
    for (const a of p.acts ?? []) if (a.kind === "speed") speed = a.a;
    const next = i + 1 < points.length ? points[i + 1] : endsHome(plan) ? origin : null;
    if (speed != null && next) speedLegs.push({ a: p, b: next, v: speed });
  });
  return (
    <g className="mission-marks" pointerEvents="none">
      <defs>
        <marker id="mission-jump-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M 0 0 L 10 5 L 0 10 z" fill="#ce93d8" />
        </marker>
      </defs>
      {speedLegs.map((s, k) => {
        const a = xy(s.a);
        const b = xy(s.b);
        return <line key={`v${k}`} x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke="#4fc3f7" strokeWidth="5" strokeOpacity="0.28" strokeLinecap="round" />;
      })}
      {points.map((p, i) => {
        const at = xy(p);
        const kind = p.kind ?? "wp";
        const acts = p.acts ?? [];
        const out: ReactElement[] = [];
        if (sel === i) out.push(<circle key="sel" cx={at.x} cy={at.y} r="10" fill="none" stroke="#4fc3f7" strokeWidth="1.5" />);
        if (kind === "time" || kind === "turns") {
          if (vehicle === "plane" && loiterR >= 4) {
            out.push(<circle key="loit" cx={at.x} cy={at.y} r={loiterR} fill="none" stroke="#ffb74d" strokeWidth="1.2" strokeDasharray="4 4" opacity="0.8" />);
          }
          out.push(
            <text key="hold" x={at.x} y={at.y + 18} textAnchor="middle" fill="#ffb74d" fontSize="10">
              {kind === "time" ? t("{v} s", { v: p.hold ?? 0 }) : `×${p.hold ?? 0}`}
            </text>,
          );
        }
        if (kind === "land") {
          out.push(<polygon key="land" points={`${at.x - 6},${at.y - 12} ${at.x + 6},${at.y - 12} ${at.x},${at.y - 4}`} fill="#ef9a9a" />);
        }
        acts.forEach((a, j) => {
          const target = points[a.a];
          if (a.kind === "jump" && target) {
            const b = xy(target);
            const mx = (at.x + b.x) / 2;
            const my = (at.y + b.y) / 2;
            const len = Math.hypot(b.x - at.x, b.y - at.y) || 1;
            const bend = Math.max(30, len * 0.25);
            const cx = mx + ((b.y - at.y) / len) * bend;
            const cy = my - ((b.x - at.x) / len) * bend;
            out.push(
              <path
                key={`j${j}`}
                d={`M ${at.x} ${at.y} Q ${cx} ${cy} ${b.x} ${b.y}`}
                fill="none"
                stroke="#ce93d8"
                strokeWidth="1.5"
                strokeDasharray="6 3"
                markerEnd="url(#mission-jump-arrow)"
              />,
              <text key={`jt${j}`} x={(at.x + 2 * cx + b.x) / 4} y={(at.y + 2 * cy + b.y) / 4} fill="#ce93d8" fontSize="11" textAnchor="middle">
                {actBadge(a, t)}
              </text>,
            );
          }
          if (a.kind === "roi" && target) {
            const b = xy(target);
            out.push(<line key={`r${j}`} x1={at.x} y1={at.y} x2={b.x} y2={b.y} stroke="#80cbc4" strokeWidth="1" strokeDasharray="1 4" />);
          }
          if (a.kind === "distance") {
            const next = nextAfter(plan, i, origin);
            if (next) {
              const leg = dist(p, next);
              if (a.a < leg) {
                const d = toEnu(next.lat, next.lon, p);
                const k = (leg - a.a) / leg;
                const g = xy(fromEnu(d.e * k, d.n * k, p));
                const nb = xy(next);
                const ux = (nb.x - at.x) / (Math.hypot(nb.x - at.x, nb.y - at.y) || 1);
                const uy = (nb.y - at.y) / (Math.hypot(nb.x - at.x, nb.y - at.y) || 1);
                out.push(
                  <line key={`d${j}`} x1={g.x - uy * 7} y1={g.y + ux * 7} x2={g.x + uy * 7} y2={g.y - ux * 7} stroke="#ffb74d" strokeWidth="2" />,
                  <text key={`dt${j}`} x={g.x + uy * 10} y={g.y - ux * 10} fill="#ffb74d" fontSize="10">
                    {t("{v} m", { v: a.a })}
                  </text>,
                );
              }
            }
          }
        });
        if (acts.length) {
          const anchor = xy(actionAnchor(plan, i, origin, vehicle, tune, e2t));
          if (Math.hypot(anchor.x - at.x, anchor.y - at.y) > 3) {
            out.push(<circle key="anc" cx={anchor.x} cy={anchor.y} r="2.5" fill="#e8eef4" />);
          }
          const shown = acts.slice(0, 3);
          let x = anchor.x + 8;
          const y = anchor.y + 8;
          shown.forEach((a, j) => {
            const st = actState(i, j);
            const text = st?.state === "wait" && st.left ? `${actBadge(a, t)} ${st.left}` : actBadge(a, t);
            const w = text.length * 6.2 + 8;
            const fill = st?.state === "done" ? "#1d2a22" : st?.state === "wait" ? "#3a2a10" : "#1b2530";
            const ink = st?.state === "done" ? "#81c784" : st?.state === "wait" ? "#ffb74d" : ACTS[a.kind].cond ? "#b0bec5" : "#e8eef4";
            out.push(
              <g key={`b${j}`} className="mission-badge">
                <rect x={x} y={y} width={w} height="15" rx="4" fill={fill} stroke={ink} strokeOpacity="0.5" />
                <text x={x + 4} y={y + 11} fill={ink} fontSize="10">
                  {text}
                </text>
              </g>,
            );
            x += w + 3;
          });
          if (acts.length > shown.length) {
            out.push(
              <text key="more" x={x + 2} y={y + 11} fill="#8b98a8" fontSize="10">
                +{acts.length - shown.length}
              </text>,
            );
          }
        }
        return <g key={i}>{out}</g>;
      })}
    </g>
  );
}

function HomeMark({ at, hot }: { at: { x: number; y: number }; hot: boolean }) {
  const s = 5;
  return (
    <polygon
      points={`${at.x},${at.y - s} ${at.x + s},${at.y} ${at.x},${at.y + s} ${at.x - s},${at.y}`}
      fill={hot ? "#4fc3f7" : "#8b98a8"}
    />
  );
}
