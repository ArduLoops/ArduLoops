import { useCallback, useEffect, useMemo, useState } from "react";
import { useT } from "../i18n/i18n";
import { openAssistantChat } from "../lib/assistantAsk";
import { fetchBoardChats, fetchBoardParams, fetchBoards, type BoardChat, type BoardRow } from "../lib/boards";
import { markShape, splitNote } from "../lib/vehicleHeader";
import { hasProfile, profileChips, readProfile } from "../lib/vehicleProfile";
import { NoteEditor, ProfileEditor, VehicleMark, type Controller } from "./ControllerBoard";
import { VehicleLogs } from "./VehicleLogs";

const WHEN = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
const TABS = ["params", "logs", "chats"] as const;
type Tab = (typeof TABS)[number];
const TAB_LABEL: Record<Tab, string> = { params: "Params", logs: "Logs", chats: "Chats" };

function when(seconds: number): string {
  return seconds > 0 ? WHEN.format(new Date(seconds * 1000)) : "—";
}

function shortUid(uid: string): string {
  return uid.length > 12 ? `${uid.slice(0, 8)}…${uid.slice(-4)}` : uid;
}

/**
 * A board from the database, not on the link: its note and profile (editable), the parameters saved
 * the last time it was linked (read only), its logs on this computer and its assistant chats. Nothing
 * here writes to a vehicle.
 */
export function BoardView({ boardKey, onClose }: { boardKey: string; onClose: () => void }) {
  const t = useT();
  const [row, setRow] = useState<BoardRow | null>(null);
  const [missing, setMissing] = useState(false);
  const [tab, setTab] = useState<Tab>("params");
  const [saved, setSaved] = useState<{ saved_at: number; params: Record<string, number> } | null>(null);
  const [chats, setChats] = useState<BoardChat[]>([]);
  const [needle, setNeedle] = useState("");
  const [editing, setEditing] = useState<"name" | "note" | null>(null);
  const [profileOpen, setProfileOpen] = useState(false);

  const reload = useCallback(() => {
    void fetchBoards()
      .then((rows) => {
        const found = rows.find((r) => r.key === boardKey) ?? null;
        setRow(found);
        setMissing(!found);
      })
      .catch(() => setMissing(true));
  }, [boardKey]);
  useEffect(() => {
    setSaved(null);
    setChats([]);
    reload();
    void fetchBoardParams(boardKey).then(setSaved).catch(() => setSaved({ saved_at: 0, params: {} }));
    void fetchBoardChats(boardKey).then(setChats).catch(() => setChats([]));
  }, [boardKey, reload]);

  const names = useMemo(() => {
    const all = Object.keys(saved?.params ?? {}).sort();
    const q = needle.trim().toUpperCase();
    return q ? all.filter((name) => name.includes(q)) : all;
  }, [saved, needle]);

  if (missing) {
    return (
      <section className="scope work board-view">
        <p className="bv-empty">{t("This board is not in the database.")}</p>
        <button type="button" className="bv-close" onClick={onClose}>{t("Close")}</button>
      </section>
    );
  }
  if (!row) return <section className="scope work board-view"><p className="bv-empty">{t("Loading…")}</p></section>;

  const vehicle = (row.vehicle === "plane" ? "plane" : row.vehicle === "copter" ? "copter" : "") as "copter" | "plane" | "";
  const note = splitNote(row.comment);
  const shape = markShape(vehicle, saved?.params);
  const profile = readProfile(row.profile);
  const chips = profile ? profileChips(profile, vehicle, t) : [];
  const controller: Controller = {
    key: row.key,
    uid: row.key,
    board_id: row.board_id,
    board_name: row.board_name,
    vehicle_id: row.vehicle,
    system_id: 1,
    comment: row.comment,
    profile: row.profile,
  };
  const boardLine = [row.board_name || t("Board {id}", { id: row.board_id }), vehicle === "plane" ? "Plane" : vehicle === "copter" ? "Copter" : ""]
    .filter(Boolean)
    .join(" · ");

  return (
    <section className="scope work board-view">
      <div className="vboard">
        <div className="vb-in">
          <div className="vb-id">
            <span className="vb-mark"><VehicleMark shape={shape} /></span>
            <div className="vb-text-col">
              <div className="vb-line">
                {note.title ? <b className="vb-title" title={note.title}>{note.title}</b>
                  : <button type="button" className="vb-title ghost" onClick={() => setEditing("name")}>{t("Name this vehicle")}</button>}
                <span className="vb-state off">{t("Not on the link")}</span>
                <button type="button" className="vb-pencil" onClick={() => setEditing(note.title ? "note" : "name")} title={t("Edit note")} aria-label={t("Edit note")}>
                  <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M10.5 2.8l2.7 2.7-7.6 7.6H2.9v-2.7z" /></svg>
                </button>
                <span className="vb-board" title={row.key}>{boardLine} · {shortUid(row.key)}</span>
              </div>
              {note.body ? <p className="vb-note" title={note.body}>{note.body}</p>
                : !note.title ? <p className="vb-note ghost">{t("Add a note: what this vehicle is, what changed, what to watch.")}</p> : null}
              <div className="vb-chips">
                {chips.map((text) => (
                  <button type="button" key={text} className="vb-chip prof" title={t("From your vehicle profile")} onClick={() => setProfileOpen(true)}>{text}</button>
                ))}
                {hasProfile(profile) ? null : (
                  <button type="button" className="vb-chip add" title={t("Props, cells, weight and how it flies")} onClick={() => setProfileOpen(true)}>
                    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1.5l1.6 4.9 4.9 1.6-4.9 1.6L8 14.5l-1.6-4.9L1.5 8l4.9-1.6z" /></svg>
                    {t("Vehicle profile")}
                  </button>
                )}
              </div>
            </div>
            {profileOpen ? <ProfileEditor controller={controller} blocked="" onClose={() => setProfileOpen(false)} onSaved={() => { setProfileOpen(false); reload(); }} /> : null}
            {editing ? <NoteEditor initial={row.comment} focus={editing} controllerKey={row.key} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); reload(); }} /> : null}
          </div>
          <dl className="bv-facts">
            <div><dt>{t("Last on the link")}</dt><dd>{when(row.last_seen)}</dd></div>
            <div><dt>{t("Saved parameters")}</dt><dd>{row.params ? `${row.params} · ${when(row.params_at)}` : "—"}</dd></div>
            <div><dt>{t("Logs here")}</dt><dd>{row.logs}</dd></div>
            <div><dt>{t("Chats")}</dt><dd>{row.chats}</dd></div>
          </dl>
          <button type="button" className="bv-close" onClick={onClose}>{t("Close")}</button>
        </div>
      </div>
      <div className="studio-tabs controller-tabs" role="tablist" aria-label={t("Board")}>
        {TABS.map((id) => (
          <button key={id} type="button" role="tab" aria-selected={tab === id} onClick={() => setTab(id)}>
            <span>{t(TAB_LABEL[id])}</span>
          </button>
        ))}
      </div>
      {tab === "params" ? (
        <div className="controller-pane bv-params">
          <div className="bv-bar">
            <input className="bv-search" type="search" value={needle} placeholder={t("Search parameters")} aria-label={t("Search parameters")} onChange={(ev) => setNeedle(ev.target.value)} />
            <span className="bv-dim">
              {saved && saved.saved_at
                ? t("Saved {when}, when the board was last linked. Read only.", { when: when(saved.saved_at) })
                : saved ? t("No parameters were saved for this board.") : t("Loading…")}
            </span>
          </div>
          <div className="bv-table" role="table" aria-label={t("Saved parameters")}>
            {names.map((name) => (
              <div className="bv-row" role="row" key={name}>
                <span role="cell">{name}</span>
                <span role="cell" className="r">{formatValue(saved!.params[name])}</span>
              </div>
            ))}
          </div>
        </div>
      ) : null}
      {tab === "logs" ? (
        <div className="controller-pane">
          <VehicleLogs active board={row.key} />
        </div>
      ) : null}
      {tab === "chats" ? (
        <div className="controller-pane bv-chats">
          {chats.length === 0 ? <p className="bv-empty">{t("No assistant chats for this board.")}</p> : chats.map((chat) => (
            <button type="button" key={chat.id} className="bv-chat" onClick={() => openAssistantChat(chat.id)}>
              <span>{chat.title}</span>
              <small>{when(chat.updated_at)}</small>
            </button>
          ))}
        </div>
      ) : null}
    </section>
  );
}

function formatValue(value: number | undefined): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(7)));
}
