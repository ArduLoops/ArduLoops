import { useEffect, useState } from "react";
import { useT } from "../i18n/i18n";
import { openAssistant } from "../lib/assistantAsk";
import { APP_HTTP } from "../mav/link";
import { FloatWindow } from "./FloatWindow";

/** The findings of the local log tools for one stored log (/logs/local rows, /logs/findings), and their window. */

export type Tr = (key: string, vars?: Record<string, string | number>) => string;
export type FlagKind = "crash" | "err" | "fs" | "batt" | "clip";
export type Flag = { kind: FlagKind; count: number; at_s: number; text: string };
export type ToolArgs = { axis?: string; start_us?: number; end_us?: number };
/** A rule a local log tool fired on this log, in the tool's own words and numbers. */
export type Finding = { tool: string; severity: "critical" | "warning" | "info"; kind: string; text: string; args: ToolArgs };
export type ToolCall = { tool: string; args: ToolArgs; ok: boolean; error?: string; skipped?: boolean };
export type Findings = { findings: Finding[]; calls: ToolCall[] };

export const FLAG_LABEL: Record<FlagKind, string> = { crash: "Crash", err: "Errors", fs: "Failsafe", batt: "Low battery", clip: "Clipping" };
export const HOT = new Set<FlagKind>(["crash", "err"]);
/** The WebTools review each local log tool does (motor balance is ArduLoops' own). Product names stay untranslated. */
const TOOL_LABEL: Record<string, string> = { log_audit: "HardwareReport", log_filter_review: "FilterReview", log_motor_balance: "Motor balance", log_magfit: "MAGFit", log_pid_review: "PIDReview" };
export const toolLabel = (tool: string, t: Tr) => (tool === "log_motor_balance" ? t(TOOL_LABEL[tool]) : TOOL_LABEL[tool] ?? tool);

export function spanText(seconds: number, t: Tr): string {
  const total = Math.max(1, Math.round(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h) return t("{h} h {m} min", { h, m });
  if (m && s) return t("{m} min {s} s", { m, s });
  if (m) return t("{m} min", { m });
  return t("{s} s", { s });
}

/** The arguments of a tool call as the assistant would write them: file, axis, interval. */
function callText(tool: string, args: ToolArgs, file: string): string {
  const parts = [`file=${file}`];
  if (args.axis) parts.push(`axis=${args.axis}`);
  if (args.start_us != null) parts.push(`start_us=${args.start_us}`);
  if (args.end_us != null) parts.push(`end_us=${args.end_us}`);
  return `${tool}(${parts.join(", ")})`;
}

/** Axis and interval of a finding: "pitch · 54.0–83.0 s". */
export function argsText(args: ToolArgs): string {
  const parts: string[] = [];
  if (args.axis) parts.push(args.axis);
  if (args.start_us != null) parts.push(`${(args.start_us / 1e6).toFixed(1)}–${args.end_us != null ? (args.end_us / 1e6).toFixed(1) : "…"} s`);
  return parts.join(" · ");
}

/** Opens the assistant with the log attached and a message, not sent, that carries the findings and the calls already made. */
export function askAboutFindings(id: string, found: Findings | null | undefined, t: Tr): void {
  if (!found) {
    openAssistant([t("What happened in this flight?"), t("Is anything in this log worth fixing?")], id);
    return;
  }
  const lines = [t("ArduLoops ran the local log tools on this log. Findings:")];
  for (const f of found.findings) lines.push(`- ${callText(f.tool, f.args, id)}: ${f.text}`);
  lines.push("", t("Tool calls already made, with their results attached:"));
  for (const c of found.calls) lines.push(`- ${callText(c.tool, c.args, id)}${c.ok ? "" : ` — ${c.skipped ? `${t("Not run")}: ` : ""}${c.error ?? t("no result")}`}`);
  lines.push("", t("What do these findings mean for this vehicle?"));
  openAssistant([], id, lines.join("\n"));
}

type CallDetail = { tool: string; args: ToolArgs & { file?: string }; result: Record<string, any> };
type FindingsDetail = { file: string; findings: Finding[]; calls: CallDetail[] };

const r1 = (v: unknown) => (typeof v === "number" ? +v.toFixed(1) : null);

/** The numbers of one tool call that the findings window shows, from the tool's own result. */
function callNumbers(call: CallDetail, t: Tr): string {
  const r = call.result;
  if (r.skipped) return `${t("Not run")}: ${r.error ?? ""}`;
  if (r.ok === false) return r.not_measured || r.error || t("no result");
  const parts: string[] = [];
  const add = (label: string, value: unknown, unit = "") => {
    const n = r1(value);
    if (n != null) parts.push(`${t(label)} ${n}${unit}`);
  };
  if (call.tool === "log_audit") {
    if (r.health) parts.push(`${t("Health")} ${r.health.status} · ${r.health.score}`);
    const v = r.vibrations ?? {};
    const vmax = Math.max(v.vibe_x_max ?? 0, v.vibe_y_max ?? 0, v.vibe_z_max ?? 0);
    if (vmax) parts.push(`${t("Vibration")} ${r1(vmax)} m/s²`);
    add("Vcc min", r.power?.vcc_min, " V");
    add("Battery min", r.power?.battery_min_volt_armed, " V");
  } else if (call.tool === "log_filter_review") {
    add("Noise peak", r.primary_peak_hz, " Hz");
    add("Notch at", r.tracked_notch_hz, " Hz");
    const s = r.simulation ?? {};
    if (s.current_attenuation_db != null) {
      const now = `${r1(s.current_attenuation_db)} dB`;
      parts.push(`${t("Attenuation")} ${r.needs_update ? `${now} → ${r1(s.proposed_attenuation_db)} dB` : now}`);
    }
    if (s.current_phase_delay_ms != null) {
      const now = `${r1(s.current_phase_delay_ms)} ms`;
      parts.push(`${t("Delay at 30 Hz")} ${r.needs_update ? `${now} → ${r1(s.proposed_phase_delay_ms)} ms` : now}`);
    }
  } else if (call.tool === "log_motor_balance") {
    for (const m of r.motors ?? []) parts.push(`M${m.motor} ${m.diff_pct > 0 ? "+" : ""}${r1(m.diff_pct)} %`);
    if (r.motors_count === 4) add("Yaw twist", r.balance?.yaw_twist_pct, " %");
  } else if (call.tool === "log_magfit") {
    if (r.orig_error_mgauss != null) parts.push(`${t("Field error")} ${r1(r.orig_error_mgauss)} → ${r1(r.fit_error_mgauss)} mGauss`);
    add("Coverage", r.coverage_pct, " %");
    if (r.orientation?.logged) parts.push(`COMPASS_ORIENT ${r.orientation.logged}${r.orientation.matches === false ? ` → ${r.orientation.best_fit}` : " ✓"}`);
  } else if (call.tool === "log_pid_review") {
    if (r.not_measured) return r.not_measured;
    const m = r.metrics ?? {};
    add("Overshoot", m.overshoot_pct, " %");
    add("Rise", m.rise_time_ms, " ms");
    add("Phase margin", m.phase_margin_deg, "°");
    add("Gain margin", m.gain_margin_db, " dB");
    add("Bandwidth", m.bandwidth_hz, " Hz");
    if (!parts.length && r.step_response_unavailable) return r.step_response_unavailable;
  }
  return parts.join(" · ") || "—";
}

/** What the findings window shows: the stored log, its scan flags and the findings. Also the payload of its detached window. */
export type FindingsData = { id: string; name: string; flags: Flag[]; findings: Findings | null; pending: boolean };

/** One log's flags and findings, with the numbers of each tool call, and the way to the assistant. */
export function FindingsWindow({ data, onClose, detached = false }: { data: FindingsData; onClose: () => void; detached?: boolean }) {
  const t = useT();
  const [detail, setDetail] = useState<FindingsDetail | null>(null);
  const [error, setError] = useState("");
  const version = data.findings ? data.findings.calls.length * 1000 + data.findings.findings.length : -1;
  useEffect(() => {
    if (!data.id || !data.findings) return;
    let stop = false;
    void fetch(`${APP_HTTP}/logs/findings?id=${encodeURIComponent(data.id)}`)
      .then(async (res) => {
        const body = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(body.error || `HTTP ${res.status}`);
        if (!stop) setDetail(body as FindingsDetail);
      })
      .catch((reason: Error) => !stop && setError(reason.message));
    return () => {
      stop = true;
    };
  }, [data.id, version]);
  const findings = data.findings?.findings ?? [];
  const title = t("Log {id} · flags and findings", { id: data.name });
  return (
    <FloatWindow
      title={title}
      className="findings"
      onClose={onClose}
      detached={detached}
      detach={{ kind: "findings", payload: data, title, key: data.id }}
    >
      <div className="log-fd-body">
        {data.flags.length ? (
          <section className="log-fd">
            <h4>{t("Flags")}</h4>
            {data.flags.map((f) => (
              <div className="log-pop-line" key={f.kind}>
                <span className={`log-flag ${HOT.has(f.kind) ? "hot" : "amber"}`}>{t(FLAG_LABEL[f.kind])}{f.count > 1 ? ` ${f.count}` : ""}</span>
                <span>{f.at_s ? `${spanText(f.at_s, t)} · ` : ""}{f.text}</span>
              </div>
            ))}
          </section>
        ) : null}
        <section className="log-fd">
          <h4>{t("Findings of the local log tools")}</h4>
          {data.pending ? <p className="log-dim">{t("The log tools are reading this log…")}</p>
            : !data.findings ? <p className="log-dim">{t("The log tools have not run on this log yet")}</p>
            : !findings.length ? <p className="log-dim">{t("No rule of the log tools fired on this log.")}</p>
            : findings.map((f, i) => (
              <div className="log-pop-line" key={i}>
                <span className={`log-flag ${f.severity === "critical" ? "hot" : "amber"}`} title={f.tool}>{toolLabel(f.tool, t)}</span>
                <span>{argsText(f.args) ? `${argsText(f.args)} · ` : ""}{f.text}</span>
              </div>
            ))}
        </section>
        {detail?.calls.length ? (
          <section className="log-fd">
            <h4>{t("Tool results")}</h4>
            {detail.calls.map((c, i) => (
              <div className={`log-fd-call${c.result.ok === false ? " off" : ""}`} key={i}>
                <span><b>{toolLabel(c.tool, t)}</b>{argsText(c.args) ? ` · ${argsText(c.args)}` : ""}<small className="mono">{c.tool}</small></span>
                <span>{callNumbers(c, t)}</span>
              </div>
            ))}
            <p className="log-dim">{t("The assistant gets these results with the log, so it does not run the same tools again.")}</p>
          </section>
        ) : error ? <p className="log-dim">{t(error)}</p> : null}
      </div>
      <footer className="log-fd-foot">
        <button type="button" className="log-act ai" onClick={() => askAboutFindings(data.id, data.findings, t)}>✦ {t("Analyze with the assistant")}</button>
      </footer>
    </FloatWindow>
  );
}
