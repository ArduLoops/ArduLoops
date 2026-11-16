import "./LivePlots.css";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useT } from "../i18n/i18n";
import { customCatalog, customPane, liveFieldSig, resolveLine, type CatalogTrace, type Pane } from "../lib/traces";
import { getLatest } from "../mav/store";
import { useStorePicked } from "../mav/view";
import { FloatWindow } from "./FloatWindow";
import { PlotStack } from "./TracePanes";

export type LiveLine = { id: string; label: string };
export type LiveChart = { title: string; lines: LiveLine[] };
export type LiveSpec = { charts: LiveChart[] };

export function liveSpecFromTools(items: { name: string; output: string }[]): LiveSpec | null {
  let spec: LiveSpec | null = null;
  for (const item of items) {
    if (item.name !== "show_live") continue;
    const next = liveSpec(item.output);
    if (next) spec = next;
  }
  return spec;
}

export function liveSpec(output: string): LiveSpec | null {
  try {
    const value = JSON.parse(output) as { ok?: boolean; charts?: unknown };
    if (!value || value.ok === false || !Array.isArray(value.charts)) return null;
    const charts: LiveChart[] = [];
    for (const row of value.charts) {
      if (!row || typeof row !== "object") continue;
      const rec = row as { title?: unknown; lines?: unknown };
      if (!Array.isArray(rec.lines)) continue;
      const lines: LiveLine[] = [];
      for (const line of rec.lines) {
        if (!line || typeof line !== "object") continue;
        const item = line as { id?: unknown; label?: unknown };
        if (typeof item.id !== "string" || !item.id.trim()) continue;
        const id = item.id.trim();
        const label = typeof item.label === "string" && item.label.trim() ? item.label.trim() : id;
        lines.push({ id, label });
      }
      if (!lines.length) continue;
      const title = typeof rec.title === "string" ? rec.title.trim() : "";
      charts.push({ title: title || lines.map((line) => line.label).join(", "), lines });
    }
    return charts.length ? { charts: charts.slice(0, 5) } : null;
  } catch {
    return null;
  }
}

function chartStamp(spec: LiveSpec): string {
  return spec.charts.map((chart) => `${chart.title}\n${chart.lines.map((line) => line.id).join(",")}`).join("\n");
}

type LiveDockApi = {
  flags: boolean[];
  setOpen: (open: boolean) => void;
  setSpec: (spec: LiveSpec | null) => void;
  toggle: (index: number, checked: boolean) => void;
};

const LiveDock = createContext<LiveDockApi | null>(null);

export function LiveHost({ chatId, children }: { chatId: string; children: ReactNode }) {
  const [spec, setSpecState] = useState<LiveSpec | null>(null);
  const [open, setOpen] = useState(false);
  const [flagState, setFlagState] = useState({ stamp: "", on: [] as boolean[] });
  const stamp = spec ? chartStamp(spec) : "";
  if (spec && flagState.stamp !== stamp) setFlagState({ stamp, on: spec.charts.map(() => true) });
  const flags = spec && flagState.stamp === stamp ? flagState.on : (spec ? spec.charts.map(() => true) : []);
  const seen = useRef(chatId);
  useEffect(() => {
    if (seen.current === chatId) return;
    seen.current = chatId;
    setOpen(false);
  }, [chatId]);
  const setSpec = useCallback((next: LiveSpec | null) => {
    setSpecState((prev) => (prev === next ? prev : next));
  }, []);
  const toggle = useCallback((index: number, checked: boolean) => {
    setFlagState((prev) => {
      const next = prev.on.slice();
      next[index] = checked;
      return { stamp: prev.stamp, on: next };
    });
  }, []);
  const api = useMemo<LiveDockApi>(() => ({ flags, setOpen, setSpec, toggle }), [flags, setSpec, toggle]);
  const shown: LiveSpec | null = spec ? { charts: spec.charts.filter((_, index) => flags[index]) } : null;
  return (
    <LiveDock.Provider value={api}>
      {children}
      {open && shown ? <LiveWindow spec={shown} onClose={() => setOpen(false)} /> : null}
    </LiveDock.Provider>
  );
}

export function LiveView({ spec }: { spec: LiveSpec }) {
  const dock = useContext(LiveDock);
  const setSpec = dock?.setSpec;
  useEffect(() => { setSpec?.(spec); }, [spec, setSpec]);
  if (!dock) return null;
  const on = dock.flags.length === spec.charts.length ? dock.flags : spec.charts.map(() => true);
  return <LiveCard spec={spec} on={on} onToggle={dock.toggle} onOpen={() => dock.setOpen(true)} />;
}

export function LiveCard({
  spec,
  on,
  onToggle,
  onOpen,
}: {
  spec: LiveSpec;
  on: boolean[];
  onToggle: (index: number, checked: boolean) => void;
  onOpen: () => void;
}) {
  const t = useT();
  return (
    <div className="ai-charts">
      <div className="ai-chart-card ai-live-card">
        <button type="button" className="ai-live-open" onClick={onOpen}>
          <b>{t("Live lines")}</b>
        </button>
        {spec.charts.map((chart, index) => (
          <div key={`${chart.title}:${index}`} className={on[index] ? "ai-live-row" : "ai-live-row off"}>
            <input
              type="checkbox"
              checked={!!on[index]}
              aria-label={chart.title}
              onChange={(ev) => onToggle(index, ev.target.checked)}
            />
            <button type="button" onClick={onOpen}>
              <b>{chart.title}</b>
              <span>{chart.lines.map((line) => line.label).join(", ")}</span>
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

/** `detached`: the window is its own OS window; it fills it and the OS moves and sizes it. */
export function LiveWindow({ spec, onClose, detached = false }: { spec: LiveSpec; onClose: () => void; detached?: boolean }) {
  const t = useT();
  const paramN = useStorePicked((s) => Object.keys(s.params).length);
  const liveSig = useStorePicked((s) => liveFieldSig(s.live_nums));
  const panes = useMemo(
    () => panesFor(spec, customCatalog(getLatest().params || {}, getLatest().live_nums)),
    [spec, paramN, liveSig],
  );

  return (
    <FloatWindow title={t("Live lines")} className="live" onClose={onClose} detached={detached} detach={{ kind: "live", payload: spec, title: t("Live lines") }}>
      {panes.length ? (
        <div className="ai-live-plots">
          <PlotStack panes={panes} />
        </div>
      ) : (
        <p className="ai-live-empty">{t("No live chart is on")}</p>
      )}
    </FloatWindow>
  );
}

function panesFor(spec: LiveSpec, catalog: CatalogTrace[]): Pane[] {
  const panes: Pane[] = [];
  for (const chart of spec.charts) {
    const picked = chart.lines
      .map((line) => resolveLine(line.id, catalog))
      .filter((tr): tr is CatalogTrace => !!tr);
    if (!picked.length) continue;
    const pane = customPane(picked);
    panes.push({ ...pane, title: chart.title || pane.title });
  }
  return panes;
}
