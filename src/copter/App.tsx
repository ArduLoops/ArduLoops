import { lazy, Suspense, useMemo, useState } from "react";
import { Aside, type LogRow } from "../components/Aside";
import { MainSplit } from "../components/MainSplit";
import { AxisSwitch } from "../components/AxisSwitch";
import { Cascade } from "../components/Cascade";
import { CopterInspect } from "../components/CopterInspect";
import { Scope } from "../components/Scope";
import { Studio } from "../components/Studio";
import { LAYERS } from "../cascade";
import { useT } from "../i18n/i18n";
import { preferredLayoutWidth } from "../lib/layout";
import { addLog } from "../log";
import { axisLabel, type Axis } from "../mav/axis";
import { CopterLoopView } from "./LoopView";
import { AuditLibrary } from "../components/AuditLibrary";
import { ControllerView } from "../components/ControllerView";
import { COPTER_WORKSPACES, WorkspaceTabs, type WorkspaceId } from "../components/WorkspaceTabs";
import { MissionView } from "../components/MissionView";
import { StudioWindowsHost } from "../components/StudioWindows";
import { copterAxis, copterPick, setStudioFocus, useStudioFocus } from "../lib/studioFocus";

const ScriptsLibrary = lazy(() => import("../components/ScriptsLibrary").then((mod) => ({ default: mod.ScriptsLibrary })));

export function CopterApp({ log }: { log: LogRow[] }) {
  return (
    <>
      <CopterWorkspaces log={log} />
      <StudioWindowsHost frame="copter" />
    </>
  );
}

function CopterWorkspaces({
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

  function onAxis(next: Axis) {
    copterAxis(next);
    addLog(t("Axis · {axis}", { axis: t(axisLabel(next)) }), "cmd");
  }

  const onPick = copterPick;

  if (workspace === "mission") {
    return (
      <MainSplit>
        <MissionView current="mission" ids={COPTER_WORKSPACES} onWorkspace={setWorkspace} />
        <Aside
          log={log}
          sel={sel}
          onSel={onPick}
          axis={axis}
          inspect={<CopterInspect sel={sel} onSel={onPick} axis={axis} showAll={showAll} />}
        />
      </MainSplit>
    );
  }

  if (workspace === "firmware") {
    return (
      <MainSplit>
        <ControllerView ids={COPTER_WORKSPACES} onWorkspace={setWorkspace} />
        <Aside
          log={log}
          sel={sel}
          onSel={onPick}
          axis={axis}
          inspect={<CopterInspect sel={sel} onSel={onPick} axis={axis} showAll={showAll} />}
        />
      </MainSplit>
    );
  }

  if (workspace === "scripts") {
    return (
      <MainSplit>
        <Suspense fallback={<section className="scope work"><p className="work-empty">…</p></section>}>
          <ScriptsLibrary current="scripts" ids={COPTER_WORKSPACES} onWorkspace={setWorkspace} />
        </Suspense>
        <Aside
          log={log}
          sel={sel}
          onSel={onPick}
          axis={axis}
          inspect={<CopterInspect sel={sel} onSel={onPick} axis={axis} showAll={showAll} />}
        />
      </MainSplit>
    );
  }

  if (workspace === "audit") {
    return (
      <MainSplit>
        <AuditLibrary current="audit" ids={COPTER_WORKSPACES} onWorkspace={setWorkspace} />
        <Aside
          log={log}
          sel={sel}
          onSel={onPick}
          axis={axis}
          inspect={<CopterInspect sel={sel} onSel={onPick} axis={axis} showAll={showAll} />}
        />
      </MainSplit>
    );
  }

  return (
    <MainSplit>
      <section className="scope">
        <Studio
          frame="copter"
          col1Default={col1Default}
          toolbar={<>
            <WorkspaceTabs current="loops" ids={COPTER_WORKSPACES} onSelect={setWorkspace} />
            <AxisSwitch axis={axis} onAxis={onAxis} />
          </>}
          scheme={<Cascade sel={sel} onSel={onPick} axis={axis} showAll={showAll} onShowAll={setShowAll} />}
          loop={(compact) => <CopterLoopView sel={sel} axis={axis} compact={compact} />}
          scope={<Scope sel={sel} onSel={onPick} axis={axis} />}
        />
      </section>
      <Aside
        log={log}
        sel={sel}
        onSel={onPick}
        axis={axis}
        inspect={<CopterInspect sel={sel} onSel={onPick} axis={axis} showAll={showAll} />}
      />
    </MainSplit>
  );
}
