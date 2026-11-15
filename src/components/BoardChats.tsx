import { useEffect, useState } from "react";
import { useT } from "../i18n/i18n";
import { openAssistantChat } from "../lib/assistantAsk";
import { fetchBoardChats, type BoardChat } from "../lib/boards";

const WHEN = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** The assistant chats made about one board on this device. A tap opens the chat. */
export function BoardChats({ boardKey, empty }: { boardKey: string; empty?: string }) {
  const t = useT();
  const [chats, setChats] = useState<BoardChat[] | null>(null);
  useEffect(() => {
    setChats(null);
    if (!boardKey) {
      setChats([]);
      return;
    }
    let live = true;
    void fetchBoardChats(boardKey).then((rows) => live && setChats(rows)).catch(() => live && setChats([]));
    return () => {
      live = false;
    };
  }, [boardKey]);
  if (chats === null) return <div className="controller-pane bv-chats"><p className="bv-empty">{t("Loading…")}</p></div>;
  return (
    <div className="controller-pane bv-chats">
      {chats.length === 0 ? <p className="bv-empty">{empty ?? t("No assistant chats for this board.")}</p> : chats.map((chat) => (
        <button type="button" key={chat.id} className="bv-chat" onClick={() => openAssistantChat(chat.id)}>
          <span>{chat.title}</span>
          <small>{chat.updated_at > 0 ? WHEN.format(new Date(chat.updated_at * 1000)) : "—"}</small>
        </button>
      ))}
    </div>
  );
}
