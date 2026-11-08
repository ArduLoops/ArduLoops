import { APP_HTTP } from "../mav/link";

/** A board this computer has seen, keyed by its MCU UID (GET /boards). */
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
};

export type BoardChat = { id: string; title: string; updated_at: number };

export async function fetchBoards(): Promise<BoardRow[]> {
  const r = await fetch(`${APP_HTTP}/boards`, { cache: "no-store" });
  const body = (await r.json()) as { boards?: BoardRow[] };
  return body.boards ?? [];
}

export async function fetchBoardParams(key: string): Promise<{ saved_at: number; params: Record<string, number> }> {
  const r = await fetch(`${APP_HTTP}/boards/params?key=${encodeURIComponent(key)}`, { cache: "no-store" });
  const body = (await r.json()) as { saved_at?: number; params?: Record<string, number> };
  return { saved_at: body.saved_at ?? 0, params: body.params ?? {} };
}

export async function fetchBoardChats(key: string): Promise<BoardChat[]> {
  const r = await fetch(`${APP_HTTP}/boards/chats?key=${encodeURIComponent(key)}`, { cache: "no-store" });
  const body = (await r.json()) as { chats?: BoardChat[] };
  return body.chats ?? [];
}

/** The board's name for a person: the note's first line, else the board name, else its board id. */
export function boardTitle(row: Pick<BoardRow, "comment" | "board_name" | "board_id">, splitTitle: (c: string) => string): string {
  return splitTitle(row.comment) || row.board_name || `Board ${row.board_id}`;
}
