import { useEffect, useRef, useState } from "react";
import { useT } from "../i18n/i18n";
import { fetchBoardSync, removeBoardFromCloud, setBoardSync, syncBoardsNow, type BoardCloud, type BoardRow, type BoardSync } from "../lib/boards";

const WHEN = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** One short line for where a board stands with the account. */
export function boardCloudLabel(state: BoardCloud | undefined): string {
  switch (state) {
    case "synced": return "In your account";
    case "pending": return "Waiting to sync";
    case "removed": return "Removed from your account";
    case "off": return "Board sync is off";
    case "cloud_only": return "From your account";
    default: return "Sign in to sync";
  }
}

/**
 * The board sync switch, shared by a board's Cloud dialog and the Logs tab's Cloud dialog.
 * On by default when signed in, like log stars and notes.
 */
export function BoardSyncSection({ sync, busy, onToggle }: { sync: BoardSync | null; busy?: boolean; onToggle: (on: boolean) => void }) {
  const t = useT();
  if (sync && !sync.signed_in) {
    return (
      <>
        <div className="log-sect">{t("Boards")}</div>
        <p className="log-dim">{t("Sign in from Options to see your boards, their notes and saved parameters on your other devices.")}</p>
      </>
    );
  }
  return (
    <>
      <div className="log-sect">{t("Boards")}</div>
      <label className={`log-opt${busy || !sync ? " off" : ""}`}>
        <input type="checkbox" checked={sync?.enabled ?? true} disabled={busy || !sync} onChange={(ev) => onToggle(ev.target.checked)} />
        <span>
          <b>{t("Sync boards with your account")}</b>
          <small>{t("Each board's name, note, vehicle profile and last saved parameters, so your other devices show them. A few KB per note, about 40 KB per parameter set.")}</small>
        </span>
      </label>
      {sync?.enabled && sync.count != null && sync.limit ? (
        <p className="log-dim">{t("{n} of {limit} boards in your account", { n: sync.count, limit: sync.limit })}</p>
      ) : null}
    </>
  );
}

/**
 * Where one board stands with the account, the sync switch, and removing the board from the account.
 * Nothing here talks to a vehicle; the account keeps snapshots, never sends them to a board.
 */
export function BoardCloudDialog({ row, onClose, onChanged }: { row: BoardRow; onClose: () => void; onChanged: (removed: boolean) => void }) {
  const t = useT();
  const [sync, setSync] = useState<BoardSync | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState("");

  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    void fetchBoardSync().then(setSync).catch(() => setSync(null));
    const key = (ev: KeyboardEvent) => { if (ev.key === "Escape") close.current(); };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, []);

  async function run<R>(job: () => Promise<R>, after?: (out: R) => void) {
    setBusy(true);
    setError("");
    try {
      const out = await job();
      after?.(out);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Could not reach your account");
    } finally {
      setBusy(false);
    }
  }

  const fromCloud = row.source === "cloud";
  const inAccount = fromCloud || row.cloud === "synced" || row.cloud === "pending";
  const state = fromCloud ? t("From your account. It was never linked on this device, so it is read only here.")
    : row.cloud === "synced" ? t("In your account. Your other devices show this board.")
    : row.cloud === "pending" ? t("Changes here go to your account at the next sync.")
    : row.cloud === "removed" ? t("Removed from your account. The copy on this device stays. Linking the board again or editing its note puts it back.")
    : row.cloud === "off" ? t("Board sync is off. This board stays on this device.")
    : t("Signed out. This board stays on this device until you sign in.");

  return (
    <div className="log-scrim" onPointerDown={(ev) => { if (ev.target === ev.currentTarget) onClose(); }}>
      <div className="log-dlg" role="dialog" aria-modal="true" aria-label={t("Cloud")}>
        <div className="log-dlg-head"><b>{t("Cloud")}</b><button type="button" className="log-x" aria-label={t("Close")} onClick={onClose}>×</button></div>
        <div className="log-dlg-body">
          <p>{state}</p>
          {sync?.enabled && sync.synced_at ? (
            <p className="log-dim">{t("Last sync {when}", { when: WHEN.format(new Date(sync.synced_at * 1000)) })}</p>
          ) : null}
          {sync?.error && sync.enabled && sync.signed_in ? <p className="log-dim">{t("Offline or the account did not answer; showing what was saved here.")}</p> : null}
          <BoardSyncSection sync={sync} busy={busy} onToggle={(on) => void run(() => setBoardSync(on), (next) => { setSync(next); onChanged(false); })} />
          <div className="log-sect">{t("Privacy")}</div>
          <p className="log-dim">{t("What leaves this device: the board's hardware id, board type, name and note, vehicle profile, when it was last linked, and its saved parameters. Logs sync on their own; assistant chats stay on the device they were made on. Only you can read it, and nothing from the account is ever written to a vehicle.")}</p>
          {confirm ? <p className="amber">{t("Remove this board from your account for every device? The copy on each device stays.")}</p> : null}
          {error ? <p className="log-bad">{t(error)}</p> : null}
        </div>
        <div className="log-dlg-foot">
          {inAccount && sync?.signed_in ? (
            confirm ? (
              <>
                <button type="button" className="log-act ghost" disabled={busy} onClick={() => setConfirm(false)}>{t("Cancel")}</button>
                <button type="button" className="log-act danger" disabled={busy} onClick={() => void run(() => removeBoardFromCloud(row.key), () => { setConfirm(false); onChanged(true); })}>
                  {t("Remove from the cloud")}
                </button>
              </>
            ) : (
              <button type="button" className="log-act ghost" disabled={busy} onClick={() => setConfirm(true)}>{t("Remove from the cloud")}</button>
            )
          ) : null}
          <span className="log-grow" />
          {sync?.signed_in && sync.enabled ? (
            <button type="button" className="log-act ghost" disabled={busy} onClick={() => void run(syncBoardsNow, (next) => { setSync(next); onChanged(false); })}>
              {busy ? t("Syncing…") : t("Sync now")}
            </button>
          ) : null}
          <button type="button" className="log-act ghost" onClick={onClose}>{t("Close")}</button>
        </div>
      </div>
    </div>
  );
}
