/**
 * The four sticks as ArduPilot reads them: which channel each is on (RCMAP_*), the direction ArduPilot expects
 * (roll right, throttle up and yaw right raise the channel; pitch forward lowers it), and whether the ends were ever
 * calibrated (1100 to 1900 are ArduPilot's defaults).
 */
export type Stick = "roll" | "pitch" | "throttle" | "yaw";

export const STICKS: { key: Stick; name: string; map: string; fallback: number; ask: string; expect: 1 | -1; to: [number, number] }[] = [
  { key: "roll", name: "Roll", map: "RCMAP_ROLL", fallback: 1, ask: "Push roll right", expect: 1, to: [1, 0] },
  { key: "pitch", name: "Pitch", map: "RCMAP_PITCH", fallback: 2, ask: "Push pitch forward", expect: -1, to: [0, 1] },
  { key: "throttle", name: "Throttle", map: "RCMAP_THROTTLE", fallback: 3, ask: "Push throttle up", expect: 1, to: [0, 1] },
  { key: "yaw", name: "Yaw", map: "RCMAP_YAW", fallback: 4, ask: "Push yaw right", expect: 1, to: [1, 0] },
];

export function pnum(params: Record<string, number> | undefined, name: string): number | null {
  const v = params?.[name];
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

export function channelOf(params: Record<string, number> | undefined, stick: Stick): number {
  const s = STICKS.find((x) => x.key === stick)!;
  const mapped = pnum(params, s.map);
  return Math.min(16, Math.max(1, mapped != null && mapped > 0 ? Math.round(mapped) : s.fallback));
}

export type Ends = { min: number; trim: number; max: number; reversed: boolean };

export function endsOf(params: Record<string, number> | undefined, ch: number): Ends {
  return {
    min: pnum(params, `RC${ch}_MIN`) ?? 1100,
    trim: pnum(params, `RC${ch}_TRIM`) ?? 1500,
    max: pnum(params, `RC${ch}_MAX`) ?? 1900,
    reversed: (pnum(params, `RC${ch}_REVERSED`) ?? 0) !== 0,
  };
}

/** True once any of the four sticks has ends other than ArduPilot's 1100 to 1900 defaults. */
export function sticksCalibrated(params: Record<string, number> | undefined): boolean | null {
  const ends = STICKS.map((s) => {
    const ch = channelOf(params, s.key);
    return [pnum(params, `RC${ch}_MIN`), pnum(params, `RC${ch}_MAX`)];
  });
  if (ends.some(([a, b]) => a == null || b == null)) return null;
  return ends.every(([a, b]) => !(Math.round(a as number) === 1100 && Math.round(b as number) === 1900));
}

/** A raw channel value as the stick position ArduPilot uses, -1..1 (throttle 0..1 mapped to -1..1), reversal applied. */
export function stickPos(raw: number, e: Ends, stick: Stick): number {
  if (!(raw >= 800 && raw <= 2200)) return stick === "throttle" ? -1 : 0;
  if (stick === "throttle") {
    const span = e.max - e.min || 1;
    const v = ((raw - e.min) / span) * 2 - 1;
    return Math.max(-1, Math.min(1, e.reversed ? -v : v));
  }
  const v = raw >= e.trim ? (raw - e.trim) / (e.max - e.trim || 1) : (raw - e.trim) / (e.trim - e.min || 1);
  return Math.max(-1, Math.min(1, e.reversed ? -v : v));
}

/** Common RCn_OPTION values by name; anything else shows as its number. */
/**
 * The aux functions offered per vehicle when picking what a channel does. RCx_OPTION 41 is deprecated in favour of 153.
 * A board may hold others; the picker keeps a value it does not list.
 */
export const AUX_PICK: Record<"copter" | "plane", number[]> = {
  copter: [2, 3, 4, 5, 7, 9, 11, 13, 16, 17, 18, 19, 21, 22, 24, 28, 31, 32, 37, 38, 39, 40, 42, 46, 52, 55, 56, 57, 58, 61, 62, 65, 66, 68, 69, 70, 71, 72, 73, 81, 83, 84, 90, 94, 102, 153, 154, 165, 167, 212, 213, 214],
  plane: [4, 9, 11, 16, 24, 28, 31, 46, 55, 56, 58, 62, 65, 66, 72, 81, 90, 94, 102, 106, 107, 108, 153, 165, 167, 210, 212, 213, 214],
};

/** RCx_OPTION names as the firmware's parameter docs (and Mission Planner) give them; not translated. */
export const AUX_NAMES: Record<number, string> = {
  2: "Flip", 3: "Simple Mode", 4: "RTL", 5: "Save Trim", 7: "Save WP", 9: "Camera Trigger", 11: "Fence", 13: "Super Simple Mode",
  16: "Auto", 17: "AutoTune", 18: "Land", 19: "Gripper", 21: "Parachute Enable", 22: "Parachute Release", 24: "Auto Mission Reset",
  28: "Relay On/Off", 31: "Motor Emergency Stop", 32: "Motor Interlock", 37: "Throw", 38: "ADSB Avoidance En", 39: "PrecLoiter",
  40: "Proximity Avoidance", 41: "ArmDisarm (4.1 and lower)", 42: "SmartRTL", 46: "RC Override Enable", 52: "Acro", 55: "Guided",
  56: "Loiter", 57: "Follow", 58: "Clear Waypoints", 61: "ZigZag", 62: "Compass Learn", 65: "GPS Disable", 66: "Relay2 On/Off",
  68: "Stabilize", 69: "PosHold", 70: "AltHold", 71: "FlowHold", 72: "Circle", 73: "Drift", 81: "Disarm", 83: "ZigZag Auto",
  84: "Air Mode", 90: "EKF Source Set", 94: "VTX Power", 100: "KillIMU1", 101: "KillIMU2", 102: "Camera Mode Toggle",
  105: "GPS Disable Yaw", 106: "Disable Airspeed Use", 107: "Enable FW Autotune", 108: "QRTL", 153: "ArmDisarm (4.2)",
  154: "ArmDisarm with AirMode (4.2)", 165: "Arm/Emergency Motor Stop", 167: "Camera Zoom", 210: "Airbrakes",
  212: "Mount1 Roll", 213: "Mount1 Pitch", 214: "Mount1 Yaw",
};
