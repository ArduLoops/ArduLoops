import { useT } from "../i18n/i18n";
import { navigate, useNavParam, wsParam } from "../lib/nav";
import { isSitl } from "../mav/sim";
import { useStorePicked } from "../mav/view";

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

/**
 * The tab shown when the URL names none: controller for a real board on the link, loops otherwise
 * (SITL, or no vehicle). Until the parameters say whether the board is SITL, it is loops.
 */
function useDefaultWorkspace(): WorkspaceId {
  return useStorePicked((s) => {
    const sitl = isSitl(s.params) || !!s.sitl_running;
    const known = sitl || Object.keys(s.params || {}).length > 0;
    return s.ok && known && !sitl ? "firmware" : "loops";
  });
}

/** The workspace in the URL (`?ws=`). No id, or one this frame does not have, shows the default tab. */
export function useWorkspace(ids: WorkspaceId[]): [WorkspaceId, (id: WorkspaceId) => void] {
  const raw = useNavParam("ws");
  const fallback = useDefaultWorkspace();
  const current = ids.find((id) => id === raw) ?? fallback;
  // A picked tab goes in the URL, so it stays put when the default changes with the link.
  const select = (id: WorkspaceId) => navigate({ ws: wsParam(id) });
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
