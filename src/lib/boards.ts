import { APP_HTTP } from "../mav/link";

/**
 * Where a board stands with the account: synced, pending (not up yet), removed (from the account;
 * the copy here stays), off (sync off), signed_out, or cloud_only (in the account, never linked here).
 */
export type BoardCloud = "synced" | "pending" | "removed" | "off" | "signed_out" | "cloud_only";

/**
 * A board this device has seen, keyed by its MCU UID (GET /boards), or, with `source: "cloud"`, a
 * board from the account that was never linked here (read only).
 */
export type BoardRow = {
  key: string;
  board_id: number;
  board_name: string;
  vehicle: string;
  comment: string;
  profile: unknown;
  first_seen: number;
  last_seen: number;
  linked: boolean;
  params: number;
  params_at: number;
  logs: number;
  chats: number;
  source?: "local" | "cloud";
  cloud?: BoardCloud;
  /** The newest saved parameters came from another device. */
  params_source?: "local" | "cloud";
  /** Last on a link on any device of the account. */
  last_seen_anywhere?: number;
};

export type BoardSync = {
  signed_in: boolean;
  enabled: boolean;
  synced_at: number | null;
  error: string | null;
  count: number | null;
  limit: number | null;
};

export type SavedParams = { saved_at: number; params: Record<string, number>; source: "local" | "cloud" };

export type BoardChat = { id: string; title: string; updated_at: number };

export async function fetchBoards(): Promise<BoardRow[]> {
  const r = await fetch(`${APP_HTTP}/boards`, { cache: "no-store" });
  const body = (await r.json()) as { boards?: BoardRow[] };
  return body.boards ?? [];
}

/** The newest saved parameters: from this device, or from the account when another device saved later ones. */
export async function fetchBoardParams(key: string): Promise<SavedParams> {
  const r = await fetch(`${APP_HTTP}/boards/params?key=${encodeURIComponent(key)}`, { cache: "no-store" });
  const body = (await r.json()) as { saved_at?: number; params?: Record<string, number>; source?: string };
  return { saved_at: body.saved_at ?? 0, params: body.params ?? {}, source: body.source === "cloud" ? "cloud" : "local" };
}

async function boardCloud<R>(path: string, body?: unknown): Promise<R> {
  const r = await fetch(`${APP_HTTP}/boards/cloud${path}`, body === undefined
    ? { cache: "no-store" }
    : { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const out = (await r.json().catch(() => ({}))) as R & { error?: string };
  if (!r.ok) throw new Error(out.error || `HTTP ${r.status}`);
  return out;
}

export const fetchBoardSync = () => boardCloud<BoardSync>("");
export const setBoardSync = (enabled: boolean) => boardCloud<BoardSync>("/settings", { enabled });
export const syncBoardsNow = () => boardCloud<BoardSync>("/sync", {});
/** Removes the board from the account for every device. The copy on each device stays. */
export const removeBoardFromCloud = (key: string) => boardCloud<{ ok: boolean }>("/remove", { key });

export async function fetchBoardChats(key: string): Promise<BoardChat[]> {
  const r = await fetch(`${APP_HTTP}/boards/chats?key=${encodeURIComponent(key)}`, { cache: "no-store" });
  const body = (await r.json()) as { chats?: BoardChat[] };
  return body.chats ?? [];
}

/** The board's name for a person: the note's first line, else the board name, else its board id. */
export function boardTitle(row: Pick<BoardRow, "comment" | "board_name" | "board_id">, splitTitle: (c: string) => string): string {
  return splitTitle(row.comment) || row.board_name || `Board ${row.board_id}`;
}
