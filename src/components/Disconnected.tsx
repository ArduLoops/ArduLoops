import "./Disconnected.css";
import { useEffect, useMemo, useState } from "react";
import { ai, type Chat } from "../assistant/api";
import { useT } from "../i18n/i18n";
import { openAssistantChat } from "../lib/assistantAsk";
import { boardTitle, fetchBoards, type BoardRow } from "../lib/boards";
import { navigate, navHref, type Nav } from "../lib/nav";
import { markShape, splitNote } from "../lib/vehicleHeader";
import { addLog } from "../log";
import { send } from "../mav/cmd";
import { APP_HTTP, formatLink, linkLabel, loadLinkHistory, parseLink, saveLink } from "../mav/link";
import { Orb } from "./Assistant";
import { VehicleMark } from "./ControllerBoard";
import { startSitl } from "./SimRail";
import { FLAG_LABEL, HOT, type Findings, type Flag } from "./LogFindings";

const DATE_SHORT = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

const TELEGRAM_URL = "https://t.me/ArduLoops";
const YOUTUBE_URL = "https://www.youtube.com/@ArduLoops";

type StoredLog = {
  id: string;
  vehicle: string;
  onboard_id?: number | null;
  saved_at?: string | null;
  flight_utc?: string | null;
  flight_stamp?: string;
  original_name?: string;
  scan?: {
    start_utc?: string;
    armed_s?: number | null;
    flights?: number | null;
    flags?: Flag[];
  } | null;
  findings?: Findings | null;
};

function bootKey(uid: string): string {
  return uid.replace(/[^0-9A-Za-z]/g, "");
}

function readRecentLinks(): string[] {
  const hist = loadLinkHistory();
  try {
    const last = (localStorage.getItem("arduloops.link") || "").trim();
    if (last && !hist.some((u) => u.toLowerCase() === last.toLowerCase())) {
      return [last, ...hist].slice(0, 3);
    }
  } catch {
    /* ignore */
  }
  return hist.slice(0, 3);
}

function goLink(ev: React.MouseEvent<HTMLAnchorElement>, nav: Nav) {
  if (ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey || ev.button !== 0) return;
  ev.preventDefault();
  navigate(nav);
}

function LogFlags({ log }: { log: StoredLog }) {
  const t = useT();
  const flags = log.scan?.flags ?? [];
  const findings = log.findings?.findings ?? [];
  const criticalFinding = findings.some((f) => f.severity === "critical");
  if (!flags.length && !findings.length) {
    return log.scan ? <span className="land-flag ok">{t("No flags")}</span> : null;
  }
  return (
    <span className="land-flags">
      {flags.map((f) => (
        <span key={f.kind} className={`land-flag ${HOT.has(f.kind) ? "hot" : "amber"}`} title={f.text}>
          {t(FLAG_LABEL[f.kind])}{f.count > 1 ? ` ${f.count}` : ""}
        </span>
      ))}
      {findings.length ? (
        <span className={`land-flag ${criticalFinding ? "hot" : "amber"}`}>
          ✦ {findings.length}
        </span>
      ) : null}
    </span>
  );
}

type Port = { name: string; label: string; ardupilot?: boolean };

/** Ports most ground stations use, offered while there is no link history. */
const TYPICAL_LINKS = ["udpin:0.0.0.0:14550", "tcpout:127.0.0.1:5760"];

function usePorts(): Port[] {
  const [ports, setPorts] = useState<Port[]>([]);
  useEffect(() => {
    let live = true;
    const load = () => {
      if (document.visibilityState !== "visible") return;
      void fetch(`${APP_HTTP}/ports`, { cache: "no-store" })
        .then((r) => (r.ok ? r.json() : Promise.reject()))
        .then((rows: unknown) => {
          if (!live || !Array.isArray(rows)) return;
          const next = rows.filter((r): r is Port => !!r && typeof r === "object" && typeof (r as Port).name === "string");
          setPorts((prev) => (JSON.stringify(prev) === JSON.stringify(next) ? prev : next));
        })
        .catch(() => {});
    };
    load();
    const timer = window.setInterval(load, 3000);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, []);
  return ports;
}

/** The device name from the port label, without the COM name it repeats. */
function portTitle(port: Port): string {
  const label = (port.label || "").trim();
  const rest = label.toLowerCase().startsWith(port.name.toLowerCase())
    ? label.slice(port.name.length).replace(/^[\s—-]+/, "")
    : label;
  return rest && rest.toLowerCase() !== port.name.toLowerCase() ? rest : "";
}

type ConnRow = { url: string; title: string; sub: string; found?: boolean };

export function Disconnected({ onOpenOptions }: { onOpenOptions?: () => void }) {
  const t = useT();
  const [boards, setBoards] = useState<BoardRow[]>([]);
  const [logs, setLogs] = useState<StoredLog[]>([]);
  const [chats, setChats] = useState<Chat[]>([]);
  const [links, setLinks] = useState<string[]>(readRecentLinks);
  const ports = usePorts();
  const android = typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);

  useEffect(() => {
    let live = true;
    void fetchBoards().then((rows) => { if (live) setBoards(rows); }).catch(() => {});
    void fetch(`${APP_HTTP}/logs/local`, { cache: "no-store" })
      .then((r) => (r.ok ? r.json() : Promise.reject()))
      .then((body: { logs?: StoredLog[] }) => { if (live && Array.isArray(body.logs)) setLogs(body.logs); })
      .catch(() => {});
    void ai.chats().then((res) => { if (live && Array.isArray(res.chats)) setChats(res.chats); }).catch(() => {});
    setLinks(readRecentLinks());
    return () => {
      live = false;
    };
  }, []);

  const latestLogByBoard = useMemo(() => {
    const map = new Map<string, StoredLog>();
    for (const log of logs) {
      const k = log.vehicle;
      if (!k) continue;
      const cur = map.get(k);
      if (!cur) {
        map.set(k, log);
        continue;
      }
      // Prefer a log that has flags or findings if the very last log was a 0s disarmed boot
      const curHas = (cur.scan?.flags?.length ?? 0) + (cur.findings?.findings?.length ?? 0) > 0 || (cur.scan?.armed_s ?? 0) > 0;
      const nextHas = (log.scan?.flags?.length ?? 0) + (log.findings?.findings?.length ?? 0) > 0 || (log.scan?.armed_s ?? 0) > 0;
      if (!curHas && nextHas) map.set(k, log);
    }
    return map;
  }, [logs]);

  const boardNames = useMemo(() => {
    const map = new Map<string, string>();
    for (const b of boards) {
      map.set(b.key, boardTitle(b, (c) => splitNote(c).title));
    }
    return map;
  }, [boards]);

  // A detected ArduPilot port first, then the history. With no history, the usual ports.
  const conns = useMemo(() => {
    const rows: ConnRow[] = [];
    const seen = new Set<string>();
    const add = (row: ConnRow) => {
      const key = row.url.toLowerCase();
      if (seen.has(key)) return;
      seen.add(key);
      rows.push(row);
    };
    for (const port of ports) {
      if (!port.ardupilot) continue;
      const url = formatLink("serial", `${port.name}@115200`);
      add({ url, title: t("ArduPilot on {port}", { port: port.name }), sub: portTitle(port) || "USB", found: true });
    }
    for (const url of links) add({ url, title: linkLabel(url), sub: "" });
    if (!links.length) for (const url of TYPICAL_LINKS) add({ url, title: linkLabel(url), sub: "" });
    return rows.slice(0, 4);
  }, [ports, links, t]);

  const topBoards = boards.slice(0, 3);
  const topChats = chats.slice(0, 3);

  function connectUrl(rawUrl: string) {
    const parsed = parseLink(rawUrl);
    const url = formatLink(parsed.kind, parsed.value);
    saveLink(url);
    send({ op: "connect", url });
    addLog(t("Link {url}", { url }), "cmd");
  }

  function onSitl(vehicle: "copter" | "plane") {
    startSitl(vehicle);
    addLog(t("Start") + " · SITL · " + t(vehicle), "cmd");
  }

  return (
    <main className="land-main">
      <article className="land">
        <section className="land-hero">
          <Orb />
          <h1>ArduLoops <span>{t("Assistant")}</span></h1>
          <p className="land-lead">
            {t("Ground station and AI assistant for ArduPilot copters and planes.")}
          </p>
          {onOpenOptions ? (
            <button type="button" className="land-btn primary" onClick={onOpenOptions}>
              {t("Set up Assistant")}
            </button>
          ) : null}
          {/* The Android app has no SITL; the header hides its button there too. */}
          {android ? null : (
          <div className="land-actions">
            <button type="button" className="land-chip" onClick={() => onSitl("copter")}>
              <SitlIcon />
              {t("Start copter SITL")}
            </button>
            <button type="button" className="land-chip" onClick={() => onSitl("plane")}>
              <SitlIcon />
              {t("Start plane SITL")}
            </button>
          </div>
          )}
        </section>

        <div className="land-lists">
          {conns.length > 0 ? (
            <section className="land-sec" aria-label={t("Connections")}>
              <h2 className="land-sec-title">{links.length ? t("Recent connections") : t("Connections")}</h2>
              {conns.map((c) => (
                <button key={c.url} type="button" className="land-row" onClick={() => connectUrl(c.url)} title={c.url}>
                  {c.found ? <i className="land-dot" aria-hidden="true" /> : null}
                  <span className="land-row-main inline">
                    <b>{c.title}</b>
                    {c.sub ? <small>{c.sub}</small> : null}
                  </span>
                  <span className="land-row-act">{t("Link")} →</span>
                </button>
              ))}
            </section>
          ) : null}

          {topBoards.length > 0 ? (
            <section className="land-sec" aria-label={t("Recent boards")}>
              <h2 className="land-sec-title">{t("Recent boards")}</h2>
              {topBoards.map((b) => {
                const name = boardTitle(b, (c) => splitNote(c).title);
                const veh = (b.vehicle === "plane" ? "plane" : b.vehicle === "copter" ? "copter" : "") as "copter" | "plane" | "";
                const seen = b.last_seen_anywhere ?? b.last_seen;
                const meta = [
                  b.board_name !== name ? b.board_name : "",
                  seen > 0 ? DATE_SHORT.format(new Date(seen * 1000)) : "",
                  b.logs ? t("{n} logs", { n: b.logs }) : "",
                ].filter(Boolean).join(" · ");
                const latestLog = latestLogByBoard.get(b.key) ?? latestLogByBoard.get(bootKey(b.key));
                return (
                  <a key={b.key} className="land-row" href={navHref({ board: b.key })} onClick={(ev) => goLink(ev, { board: b.key })}>
                    <span className="land-board-mark"><VehicleMark shape={markShape(veh, undefined)} /></span>
                    <span className="land-row-main">
                      <b>{name}</b>
                      {meta ? <small>{meta}</small> : null}
                    </span>
                    {latestLog ? <LogFlags log={latestLog} /> : null}
                  </a>
                );
              })}
            </section>
          ) : null}

          {topChats.length > 0 ? (
            <section className="land-sec" aria-label={t("Recent chats")}>
              <h2 className="land-sec-title">{t("Recent chats")}</h2>
              {topChats.map((c) => {
                const bName = c.vehicle_key ? boardNames.get(c.vehicle_key) : "";
                const when = c.updated_at > 0 ? DATE_SHORT.format(new Date(c.updated_at * 1000)) : "";
                return (
                  <button key={c.id} type="button" className="land-row" onClick={() => openAssistantChat(c.id)}>
                    <span className="land-row-main">
                      <b>{c.title || t("New chat")}</b>
                      {bName ? <small>{bName}</small> : null}
                    </span>
                    {when ? <span className="land-row-when">{when}</span> : null}
                  </button>
                );
              })}
            </section>
          ) : null}
        </div>

        <div className="land-social" aria-label={t("Community")}>
          <a className="land-soc-link" href={TELEGRAM_URL} target="_blank" rel="noreferrer" title="Telegram">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true">
              <path d="M21.5 4.4 2.9 11.6c-1.2.5-1.2 1.2-.2 1.5l4.8 1.5 11-7c.5-.3 1-.1.6.2l-8.9 8.1-.3 4.9c.5 0 .7-.2 1-.5l2.3-2.2 4.8 3.5c.9.5 1.5.2 1.7-.8l3.1-14.7c.3-1.3-.5-1.9-1.3-1.7z" />
            </svg>
            <span>Telegram</span>
          </a>
          <a className="land-soc-link" href={YOUTUBE_URL} target="_blank" rel="noreferrer" title="YouTube">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true">
              <path d="M23 7.2a3 3 0 0 0-2.1-2.1C19 4.6 12 4.6 12 4.6s-7 0-8.9.5A3 3 0 0 0 1 7.2 31 31 0 0 0 .5 12a31 31 0 0 0 .5 4.8 3 3 0 0 0 2.1 2.1c1.9.5 8.9.5 8.9.5s7 0 8.9-.5a3 3 0 0 0 2.1-2.1A31 31 0 0 0 23.5 12a31 31 0 0 0-.5-4.8zM9.7 15.1V8.9l6 3.1-6 3.1z" />
            </svg>
            <span>YouTube</span>
          </a>
        </div>
      </article>
    </main>
  );
}

function SitlIcon() {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M9 4v16" />
    </svg>
  );
}
