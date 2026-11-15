import { useT } from "../i18n/i18n";
import { copterAxis, copterPick, planeAxis, planePick, returnStudioPane, setStudioFocus, usePoppedPanes, useStudioFocus, type StudioPane } from "../lib/studioFocus";
import { CopterLoopView } from "../copter/LoopView";
import { PlaneAxisSwitch } from "../plane/AxisSwitch";
import { PlaneLoopView } from "../plane/LoopView";
import { PlaneMap } from "../plane/Map";
import { AxisSwitch } from "./AxisSwitch";
import { Cascade } from "./Cascade";
import { FloatWindow } from "./FloatWindow";
import { Scope } from "./Scope";

/**
 * The loops studio's panes in windows: the map, the loop, and the scope. A pane opened in a window floats over
 * every tab of the app, and that window can go to its own OS window (DetachKind "studio") on another monitor.
 * Every copy follows one selected card and axis (lib/studioFocus.ts).
 */

export type StudioFrame = "copter" | "plane";
export type StudioPayload = { frame: StudioFrame; pane: StudioPane };

const TITLE: Record<StudioPane, string> = { scheme: "Map", loop: "Loop", scope: "Scope" };

/** The content of one pane, with the axis buttons where the pane depends on the axis. */
function PaneBody({ frame, pane }: StudioPayload) {
  const f = useStudioFocus();
  if (frame === "plane") {
    const axis = f.axis === "d" ? "roll" : f.axis;
    if (pane === "scheme") return <PlaneMap sel={f.sel} onSel={planePick} showAll={f.showAll} onShowAll={(on) => setStudioFocus({ showAll: on })} />;
    if (pane === "loop") return <PlaneLoopView sel={f.sel} axis={axis} compact={false} />;
    return <Scope sel={f.sel} onSel={planePick} axis={axis} />;
  }
  if (pane === "scheme") return <Cascade sel={f.sel} onSel={copterPick} axis={f.axis} showAll={f.showAll} onShowAll={(on) => setStudioFocus({ showAll: on })} />;
  if (pane === "loop") return <CopterLoopView sel={f.sel} axis={f.axis} compact={false} />;
  return <Scope sel={f.sel} onSel={copterPick} axis={f.axis} />;
}

function PaneAxis({ frame }: { frame: StudioFrame }) {
  const f = useStudioFocus();
  if (frame === "plane") return <PlaneAxisSwitch axis={f.axis === "d" ? "roll" : f.axis} onAxis={planeAxis} />;
  return <AxisSwitch axis={f.axis} onAxis={copterAxis} />;
}

/** One studio pane as a FloatWindow; `detached` when it fills its own OS window. */
export function StudioPaneWindow({ frame, pane, onClose, detached = false }: StudioPayload & { onClose: () => void; detached?: boolean }) {
  const t = useT();
  const title = t(TITLE[pane]);
  return (
    <FloatWindow
      title={title}
      className={`studio-win studio-${pane}`}
      head={pane === "scheme" ? null : <PaneAxis frame={frame} />}
      onClose={onClose}
      detached={detached}
      detach={{ kind: "studio", payload: { frame, pane } satisfies StudioPayload, title, key: `${frame}-${pane}` }}
    >
      <div className={`studio-win-body pane-${pane}`}>
        <PaneBody frame={frame} pane={pane} />
      </div>
    </FloatWindow>
  );
}

/** The panes this window of the app shows in floating windows. Mounted beside the workspaces, so the windows stay on every tab. */
export function StudioWindowsHost({ frame }: { frame: StudioFrame }) {
  const panes = usePoppedPanes();
  return (
    <>
      {panes.map((pane) => (
        <StudioPaneWindow key={pane} frame={frame} pane={pane} onClose={() => returnStudioPane(pane)} />
      ))}
    </>
  );
}
