import { navigate } from "./nav";

/** The header's Pre-arm tile: the Controller opens Setup on its Overview. */
export function openSetupOverview(): void {
  navigate({ ws: "controller", pane: "setup", step: null });
}
