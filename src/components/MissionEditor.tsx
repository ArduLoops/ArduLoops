import { useT } from "../i18n/i18n";
import {
  ACTS,
  MAX_ACTS,
  POINT_KINDS,
  actsFor,
  conditionClobbered,
  newAct,
  type Act,
  type ActKind,
  type LatLon,
  type Plan,
  type PointKind,
  type Vehicle,
} from "../lib/missionPlan";

type T = ReturnType<typeof useT>;

/** In flight: done, waiting on its condition (with what is left), or not yet. */
export type ActState = { state: "done" | "wait" | "pending"; left?: string } | null;

export function actText(act: Act, t: T): string {
  switch (act.kind) {
    case "speed":
      return t("Speed {v} m/s", { v: act.a });
    case "servo":
      return t("Servo {ch} to {pwm}", { ch: act.a, pwm: act.b });
    case "relay":
      return t(act.b ? "Relay {n} on" : "Relay {n} off", { n: act.a });
    case "cam_dist":
      return act.a > 0 ? t("Camera every {v} m", { v: act.a }) : t("Camera off");
    case "jump":
      return act.b < 0 ? t("Jump to point {n}, forever", { n: act.a + 1 }) : t("Jump to point {n}, {k} times", { n: act.a + 1, k: act.b });
    case "roi":
      return t("Look at point {n}", { n: act.a + 1 });
    case "land_start":
      return t("Landing starts here");
    case "delay":
      return t("Wait {v} s", { v: act.a });
    case "distance":
      return t("Wait until {v} m from the next point", { v: act.a });
    case "yaw":
      return t("Wait for heading {v}°", { v: act.a });
  }
}

/** A few characters for the map badge. */
export function actBadge(act: Act, t: T): string {
  switch (act.kind) {
    case "speed":
      return t("{v} m/s", { v: act.a });
    case "servo":
      return `S${act.a}`;
    case "relay":
      return `R${act.a}${act.b ? "+" : "−"}`;
    case "cam_dist":
      return act.a > 0 ? `CAM ${act.a}` : "CAM ×";
    case "jump":
      return act.b < 0 ? "↺∞" : `↺${act.b}`;
    case "roi":
      return "ROI";
    case "land_start":
      return "LAND▸";
    case "delay":
      return `◷${act.a}`;
    case "distance":
      return `↦${act.a}`;
    case "yaw":
      return `↻${act.a}°`;
  }
}

/**
 * Which actions a condition holds back: each runs after the nearest condition before it.
 * A jump is not held: AP_Mission follows it while picking the next point, at once.
 */
export function gatedBy(acts: Act[]): (number | null)[] {
  let cond: number | null = null;
  return acts.map((a, i) => {
    const g = a.kind === "jump" ? null : cond;
    if (ACTS[a.kind].cond) cond = i;
    return g;
  });
}

export function MissionEditor({
  plan,
  setPlan,
  vehicle,
  origin,
  sel,
  onSelect,
  livePoint,
  actState,
}: {
  plan: Plan;
  setPlan: (fn: (p: Plan) => Plan) => void;
  vehicle: Vehicle;
  origin: LatLon | null;
  sel: number | null;
  onSelect: (i: number | null) => void;
  livePoint: number | null;
  actState: (point: number, act: number) => ActState;
}) {
  const t = useT();
  const kinds = actsFor(vehicle);
  const n = plan.points.length;

  function editPoint(i: number, fn: (p: Plan["points"][number]) => Plan["points"][number]) {
    setPlan((p) => ({ ...p, points: p.points.map((pt, j) => (j === i ? fn(pt) : pt)) }));
  }

  function removePoint(i: number) {
    // Point references in jumps and ROIs shift with the list.
    setPlan((p) => ({
      ...p,
      points: p.points
        .filter((_, j) => j !== i)
        .map((pt) =>
          pt.acts
            ? {
                ...pt,
                acts: pt.acts
                  .filter((a) => !(ACTS[a.kind].a?.point && a.a === i))
                  .map((a) => (ACTS[a.kind].a?.point && a.a > i ? { ...a, a: a.a - 1 } : a)),
              }
            : pt,
        ),
    }));
    if (sel === i) onSelect(null);
    else if (sel != null && sel > i) onSelect(sel - 1);
  }

  function setKind(i: number, kind: PointKind) {
    editPoint(i, (pt) => {
      const next = { ...pt };
      if (kind === "wp") delete next.kind;
      else next.kind = kind;
      const hold = POINT_KINDS[kind].hold;
      if (hold) next.hold = hold.def;
      else delete next.hold;
      return next;
    });
  }

  function setAct(i: number, j: number, patch: Partial<Act>) {
    editPoint(i, (pt) => ({ ...pt, acts: (pt.acts ?? []).map((a, k) => (k === j ? { ...a, ...patch } : a)) }));
  }

  function addAct(i: number, kind: ActKind) {
    editPoint(i, (pt) => ({ ...pt, acts: [...(pt.acts ?? []), newAct(kind, i)].slice(0, MAX_ACTS) }));
  }

  function removeAct(i: number, j: number) {
    editPoint(i, (pt) => {
      const acts = (pt.acts ?? []).filter((_, k) => k !== j);
      const { acts: _old, ...rest } = pt;
      return acts.length ? { ...rest, acts } : rest;
    });
  }

  function moveAct(i: number, j: number, dir: -1 | 1) {
    editPoint(i, (pt) => {
      const acts = [...(pt.acts ?? [])];
      const k = j + dir;
      if (k < 0 || k >= acts.length) return pt;
      [acts[j], acts[k]] = [acts[k], acts[j]];
      return { ...pt, acts };
    });
  }

  function numInput(value: number, f: NonNullable<(typeof ACTS)[ActKind]["a"]>, onChange: (v: number) => void, label: string) {
    if (f.point) {
      return (
        <select aria-label={label} value={Math.min(value, Math.max(0, n - 1))} onChange={(ev) => onChange(Number(ev.target.value))}>
          {plan.points.map((_, k) => (
            <option key={k} value={k}>
              {k + 1}
            </option>
          ))}
        </select>
      );
    }
    return (
      <input
        type="number"
        aria-label={label}
        min={f.min}
        max={f.max}
        step={f.step}
        value={value}
        onChange={(ev) => {
          const v = Number(ev.target.value);
          if (ev.target.value.trim() !== "" && Number.isFinite(v) && v >= f.min && v <= f.max) onChange(v);
        }}
        onBlur={(ev) => {
          ev.currentTarget.value = String(value);
        }}
      />
    );
  }

  return (
    <ol className="mission-pts">
      {plan.points.map((p, i) => {
        const rel = origin ? toNE(p, origin) : null;
        const kind = p.kind ?? "wp";
        const acts = p.acts ?? [];
        const gates = gatedBy(acts);
        const open = sel === i;
        return (
          <li key={i} className={(livePoint === i ? "on " : "") + (open ? "sel" : "")}>
            <div className="mission-pt-row">
              <button type="button" className="mission-pt-name" aria-expanded={open} onClick={() => onSelect(open ? null : i)}>
                {t("Waypoint {n}", { n: i + 1 })}
                {kind !== "wp" ? ` · ${t(POINT_KINDS[kind].label)}${p.hold != null ? ` ${p.hold}` : ""}` : ""}
                {rel ? ` · ${rel.n} N ${rel.e} E` : ""}
              </button>
              <button type="button" aria-label={t("Remove waypoint")} onClick={() => removePoint(i)}>
                ×
              </button>
            </div>
            {open ? (
              <div className="mission-pt-edit">
                <label>
                  {t("Point type")}
                  <select value={kind} onChange={(ev) => setKind(i, ev.target.value as PointKind)}>
                    {(Object.keys(POINT_KINDS) as PointKind[]).map((k) => (
                      <option key={k} value={k}>
                        {t(POINT_KINDS[k].label)}
                      </option>
                    ))}
                  </select>
                </label>
                {POINT_KINDS[kind].hold ? (
                  <label>
                    {t(POINT_KINDS[kind].hold!.label)}
                    {numInput(p.hold ?? POINT_KINDS[kind].hold!.def, POINT_KINDS[kind].hold!, (v) => editPoint(i, (pt) => ({ ...pt, hold: v })), t(POINT_KINDS[kind].hold!.label))}
                  </label>
                ) : null}
              </div>
            ) : null}
            {conditionClobbered(plan, i, vehicle) ? (
              <p className="mission-hint mission-warn">
                {t("ArduPlane: the circle at the next point overwrites this condition, so the actions after it may never run.")}
              </p>
            ) : null}
            {acts.length ? (
              <ul className="mission-acts-list">
                {acts.map((a, j) => {
                  const spec = ACTS[a.kind];
                  const st = actState(i, j);
                  const cls = [
                    "mission-act",
                    spec.cond ? "cond" : "",
                    gates[j] != null ? "gated" : "",
                    st ? st.state : "",
                  ]
                    .filter(Boolean)
                    .join(" ");
                  return (
                    <li key={j} className={cls}>
                      <div className="mission-act-row">
                        <span className="mission-act-text">
                          {st?.state === "done" ? "✓ " : ""}
                          {actText(a, t)}
                          {st?.state === "wait" && st.left ? ` · ${st.left}` : ""}
                        </span>
                        {open ? (
                          <span className="mission-act-tools">
                            <button type="button" aria-label={t("Move up")} disabled={j === 0} onClick={() => moveAct(i, j, -1)}>
                              ↑
                            </button>
                            <button type="button" aria-label={t("Move down")} disabled={j === acts.length - 1} onClick={() => moveAct(i, j, 1)}>
                              ↓
                            </button>
                            <button type="button" aria-label={t("Remove action")} onClick={() => removeAct(i, j)}>
                              ×
                            </button>
                          </span>
                        ) : null}
                      </div>
                      {open && a.kind === "land_start" ? (
                        <p className="mission-hint">{t("ArduPlane will not arm with this in the mission unless RTL_AUTOLAND is set.")}</p>
                      ) : null}
                      {open && (spec.a || spec.b) ? (
                        <div className="mission-act-fields">
                          {spec.a ? (
                            <label>
                              {t(spec.a.label)}
                              {numInput(a.a, spec.a, (v) => setAct(i, j, { a: v }), t(spec.a.label))}
                            </label>
                          ) : null}
                          {spec.b ? (
                            <label>
                              {t(spec.b.label)}
                              {numInput(a.b, spec.b, (v) => setAct(i, j, { b: v }), t(spec.b.label))}
                            </label>
                          ) : null}
                        </div>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            ) : null}
            {open ? (
              <div className="mission-pt-edit">
                <select
                  aria-label={t("Add an action")}
                  value=""
                  disabled={acts.length >= MAX_ACTS}
                  onChange={(ev) => {
                    if (ev.target.value) addAct(i, ev.target.value as ActKind);
                  }}
                >
                  <option value="">{t("+ action or condition")}</option>
                  <optgroup label={t("Actions")}>
                    {kinds
                      .filter((k) => !ACTS[k].cond)
                      .map((k) => (
                        <option key={k} value={k}>
                          {t(ACTS[k].label)}
                        </option>
                      ))}
                  </optgroup>
                  <optgroup label={t("Conditions")}>
                    {kinds
                      .filter((k) => ACTS[k].cond)
                      .map((k) => (
                        <option key={k} value={k}>
                          {t(ACTS[k].label)}
                        </option>
                      ))}
                  </optgroup>
                </select>
                <p className="mission-hint">{t("Runs when this point is reached, while the vehicle flies on. A condition holds only the actions below it.")}</p>
              </div>
            ) : null}
          </li>
        );
      })}
    </ol>
  );
}

function toNE(p: LatLon, origin: LatLon): { n: number; e: number } {
  const north = (p.lat - origin.lat) * 111_320;
  const east = (p.lon - origin.lon) * 111_320 * Math.cos((origin.lat * Math.PI) / 180);
  return { n: Math.round(north), e: Math.round(east) };
}
