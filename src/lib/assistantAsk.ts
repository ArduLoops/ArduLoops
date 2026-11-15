import { detachedFromUrl, focusWindow, MAIN_LABEL, postWindowMsg } from "./detach";

/**
 * Fired by buttons outside the chat. Each starts a new conversation.
 * `send`: the text is sent at once as the user's message (Fix, Walk me through setup).
 * Otherwise the chat opens with the composer focused and nothing is sent (Ask); `starters` are offered as one-tap questions.
 */
export const ASK_ASSISTANT = "arduloops:assistant-ask";

export type AskDetail = { text: string; send: boolean; starters?: string[]; keep?: boolean; log?: string; chat?: string };

/** The Ask goes to this window's assistant, or, from a detached window without one, to the main window. */
function dispatchAsk(detail: AskDetail): void {
  const here = detachedFromUrl();
  if (here && here.kind !== "assistant") {
    postWindowMsg({ type: "main-ask", detail });
    void focusWindow(MAIN_LABEL);
    return;
  }
  window.dispatchEvent(new CustomEvent<AskDetail>(ASK_ASSISTANT, { detail }));
}

export function askAssistant(text: string): void {
  dispatchAsk({ text, send: true });
}

let focusWanted = false;

/** The composer takes focus once it is ready (an Ask forwarded to the detached assistant). */
export function requestComposerFocus(): void {
  focusWanted = true;
}

/** True once after Ask: the composer takes focus when the chat pane is ready, even if it mounts after the event. */
export function takeFocusRequest(): boolean {
  const wanted = focusWanted;
  focusWanted = false;
  return wanted;
}

/** `log` attaches a stored log to the new chat (an id from /ai/logs, or cloud:{hash}). `draft` is put in the composer, not sent. */
export function openAssistant(starters: string[] = [], log = "", draft = ""): void {
  focusWanted = true;
  dispatchAsk({ text: draft, send: false, starters, log });
}

/** Opens the assistant on one stored conversation, e.g. a chat of a board opened from the database. */
export function openAssistantChat(chat: string): void {
  dispatchAsk({ text: "", send: false, chat });
}

/** Opens the chat on the conversation already there, e.g. where an assistant card waits. */
export function showAssistant(): void {
  dispatchAsk({ text: "", send: false, keep: true });
}
