import { lazy, Suspense, useMemo, useState } from "react";
import { Aside, type LogRow } from "../components/Aside";
import { MainSplit } from "../components/MainSplit";
import { AuditLibrary } from "../components/AuditLibrary";
import { ControllerView } from "../components/ControllerView";
import { PLANE_WORKSPACES, WorkspaceTabs, type WorkspaceId } from "../components/WorkspaceTabs";
import { MissionView } from "../components/MissionView";
import { StudioWindowsHost } from "../components/StudioWindows";
import { planeAxis, planePick, setStudioFocus, useStudioFocus } from "../lib/studioFocus";

const ScriptsLibrary = lazy(() => import("../components/ScriptsLibrary").then((mod) => ({ default: mod.ScriptsLibrary })));
import { Scope } from "../components/Scope";
import { Studio } from "../components/Studio";
import { useT } from "../i18n/i18n";
import { preferredLayoutWidth } from "../lib/layout";
import { addLog } from "../log";
import { axisLabel, type Axis } from "../mav/axis";
import { PlaneAxisSwitch } from "./AxisSwitch";
import { LAYERS, MODES, NODES } from "./cascade";
import { PlaneInspect } from "./Inspect";
import { PlaneMap } from "./Map";
import { PlaneLoopView } from "./LoopView";

export function PlaneApp({ log }: { log: LogRow[] }) {
  return (
    <>
      <PlaneWorkspaces log={log} />
      <StudioWindowsHost frame="plane" />
    </>
  );
}

function PlaneWorkspaces({
  log,
}: {
  log: LogRow[];
}) {
  const t = useT();
  // The selected card and axis are the app's, so the studio's panes in windows follow them (lib/studioFocus.ts).
  const { sel, axis, showAll } = useStudioFocus();
  const setShowAll = (on: boolean) => setStudioFocus({ showAll: on });
  const [workspace, setWorkspace] = useState<WorkspaceId>("loops");
  const col1Default = useMemo(() => preferredLayoutWidth(LAYERS), []);
  const loopAxis = axis === "d" ? "roll" : axis;

  function onAxis(next: Axis) {
    planeAxis(next);
    addLog(t("Axis · {axis}", { axis: t(axisLabel(next)) }), "cmd");
  }

  const onPick = planePick;

  if (workspace === "mission") {
    return (
      <MainSplit>
        <MissionView current="mission" ids={PLANE_WORKSPACES} onWorkspace={setWorkspace} />
        <Aside
          log={log}
          sel={sel}
          onSel={onPick}
          axis={loopAxis}
          modes={MODES}
          nodes={NODES}
          presets={false}
          vehicle="plane"
          inspect={<PlaneInspect sel={sel} onSel={onPick} showAll={showAll} />}
        />
      </MainSplit>
    );
  }

  if (workspace === "firmware") {
    return (
      <MainSplit>
        <ControllerView ids={PLANE_WORKSPACES} onWorkspace={setWorkspace} />
        <Aside
          log={log}
          sel={sel}
          onSel={onPick}
          axis={loopAxis}
          modes={MODES}
          nodes={NODES}
          presets={false}
          vehicle="plane"
          inspect={<PlaneInspect sel={sel} onSel={onPick} showAll={showAll} />}
        />
      </MainSplit>
    );
  }

  if (workspace === "scripts") {
    return (
      <MainSplit>
        <Suspense fallback={<section className="scope work"><p className="work-empty">…</p></section>}>
          <ScriptsLibrary current="scripts" ids={PLANE_WORKSPACES} onWorkspace={setWorkspace} />
        </Suspense>
        <Aside
          log={log}
          sel={sel}
          onSel={onPick}
          axis={loopAxis}
          modes={MODES}
          nodes={NODES}
          presets={false}
          vehicle="plane"
          inspect={<PlaneInspect sel={sel} onSel={onPick} showAll={showAll} />}
        />
      </MainSplit>
    );
  }

  if (workspace === "audit") {
    return (
      <MainSplit>
        <AuditLibrary current="audit" ids={PLANE_WORKSPACES} onWorkspace={setWorkspace} />
        <Aside
          log={log}
          sel={sel}
          onSel={onPick}
          axis={loopAxis}
          modes={MODES}
          nodes={NODES}
          presets={false}
          vehicle="plane"
          inspect={<PlaneInspect sel={sel} onSel={onPick} showAll={showAll} />}
        />
      </MainSplit>
    );
  }

  return (
    <MainSplit>
      <section className="scope">
        <Studio
          frame="plane"
          col1Default={col1Default}
          toolbar={<>
            <WorkspaceTabs current="loops" ids={PLANE_WORKSPACES} onSelect={setWorkspace} />
            <PlaneAxisSwitch axis={loopAxis} onAxis={onAxis} />
          </>}
          scheme={<PlaneMap sel={sel} onSel={onPick} showAll={showAll} onShowAll={setShowAll} />}
          loop={(compact) => <PlaneLoopView sel={sel} axis={loopAxis} compact={compact} />}
          scope={<Scope sel={sel} onSel={onPick} axis={loopAxis} />}
        />
      </section>
      <Aside
        log={log}
        sel={sel}
        onSel={onPick}
        axis={loopAxis}
        modes={MODES}
        nodes={NODES}
        presets={false}
        vehicle="plane"
        inspect={<PlaneInspect sel={sel} onSel={onPick} showAll={showAll} />}
      />
    </MainSplit>
  );
}
