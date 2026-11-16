import { useT } from "../i18n/i18n";
import { navigate, useNavParam, wsParam } from "../lib/nav";

const TABS = [
  { id: "loops", label: "loops" },
  { id: "firmware", label: "controller" },
  { id: "mission", label: "mission" },
  { id: "scripts", label: "lua" },
  { id: "audit", label: "audit" },
] as const;

export type WorkspaceId = (typeof TABS)[number]["id"];

export const COPTER_WORKSPACES: WorkspaceId[] = ["loops", "firmware", "mission", "scripts", "audit"];
export const PLANE_WORKSPACES: WorkspaceId[] = ["loops", "firmware", "mission", "scripts", "audit"];

/** The workspace in the URL (`?ws=`). An id this frame does not have shows loops. */
export function useWorkspace(ids: WorkspaceId[]): [WorkspaceId, (id: WorkspaceId) => void] {
  const raw = useNavParam("ws");
  const current = ids.find((id) => id === raw) ?? "loops";
  const select = (id: WorkspaceId) => navigate({ ws: id === "loops" ? null : wsParam(id) });
  return [current, select];
}

export function WorkspaceTabs({
  current,
  ids,
  onSelect,
}: {
  current: WorkspaceId;
  ids: WorkspaceId[];
  onSelect: (id: WorkspaceId) => void;
}) {
  const t = useT();
  const tabs = TABS.filter((tab) => ids.includes(tab.id));
  return (
    <div className="studio-tabs" role="tablist" aria-label={t("Workspace")}>
      {tabs.map((tab) => (
        <button key={tab.id} type="button" role="tab" aria-selected={current === tab.id} onClick={() => onSelect(tab.id)}>
          <span>{t(tab.label)}</span>
        </button>
      ))}
    </div>
  );
}
