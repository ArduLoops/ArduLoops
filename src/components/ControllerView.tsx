import { startTransition, useEffect, useState } from "react";
import { OPEN_SETUP_OVERVIEW } from "../lib/setupNav";
import { useT } from "../i18n/i18n";
import { usePicked } from "../mav/view";
import { BoardChats } from "./BoardChats";
import { ControllerBoard } from "./ControllerBoard";
import { FirmwareLibrary } from "./FirmwareLibrary";
import { HardwareSetup } from "./HardwareSetup";
import { ParameterLibrary } from "./ParameterLibrary";
import { VehicleLogs } from "./VehicleLogs";
import { WorkspaceTabs, type WorkspaceId } from "./WorkspaceTabs";

const PANES = ["calibration", "logs", "params", "firmware"] as const;
/** A phone shows the board on the link like a saved board: parameters, logs, chats. */
const PHONE_PANES = ["params", "logs", "chats"] as const;
type Pane = (typeof PANES)[number] | (typeof PHONE_PANES)[number];

const TAB_LABEL: Record<Pane, string> = { calibration: "Setup", logs: "Logs", params: "Params", firmware: "Firmware", chats: "Chats" };

export function ControllerView({
  ids,
  onWorkspace,
  phone = false,
}: {
  ids?: WorkspaceId[];
  onWorkspace?: (id: WorkspaceId) => void;
  phone?: boolean;
}) {
  const t = useT();
  const panes: readonly Pane[] = phone ? PHONE_PANES : PANES;
  const [pane, setPane] = useState<Pane>(panes[0]);
  const [body, setBody] = useState<Pane>(panes[0]);
  const uid = usePicked((s) => (s.ok ? s.boot_uid ?? "" : ""));
  function choose(id: Pane) {
    if (id === pane) return;
    setPane(id);
    startTransition(() => setBody(id));
  }
  // The header's Pre-arm tile opens Setup; Setup itself turns to its Overview.
  useEffect(() => {
    if (phone) return;
    const show = () => {
      setPane("calibration");
      startTransition(() => setBody("calibration"));
    };
    window.addEventListener(OPEN_SETUP_OVERVIEW, show);
    return () => window.removeEventListener(OPEN_SETUP_OVERVIEW, show);
  }, [phone]);
  return (
    <section className="scope work">
      {phone ? null : (
        <div className="work-bar">
          <WorkspaceTabs current="firmware" ids={ids ?? []} onSelect={(id) => onWorkspace?.(id)} />
        </div>
      )}
      <ControllerBoard />
      <div className="studio-tabs controller-tabs" role="tablist" aria-label={t("controller")}>
        {panes.map((id) => (
          <button key={id} type="button" role="tab" aria-selected={pane === id} onClick={() => choose(id)}>
            <span>{t(TAB_LABEL[id])}</span>
          </button>
        ))}
      </div>
      {body === "params" ? (
        <div className="controller-pane">
          <ParameterLibrary />
        </div>
      ) : null}
      {body === "firmware" ? (
        <div className="controller-pane">
          <FirmwareLibrary />
        </div>
      ) : null}
      {body === "calibration" ? (
        <div className="controller-pane">
          <HardwareSetup />
        </div>
      ) : null}
      {body === "logs" ? (
        <div className="controller-pane">
          <VehicleLogs active />
        </div>
      ) : null}
      {body === "chats" ? <BoardChats boardKey={uid} /> : null}
    </section>
  );
}
