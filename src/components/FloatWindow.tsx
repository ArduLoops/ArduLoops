import "./FloatWindow.css";
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useT } from "../i18n/i18n";
import { detachWindow, type DetachKind } from "../lib/detach";
import { DetachButton } from "./DetachButton";

/**
 * The app's floating windows. Every window that floats over the app uses this module:
 * - `FloatWindow`: a whole window: header with title, own controls, open in separate window, full screen,
 *   close; drag by the header, resize by the corner, Escape; `detached` fills its own OS window.
 * - `useFloatDrag`, `useFloatResize`, `WindowButtons`: the same behaviour for a window that keeps its own
 *   place in state (the board doc, the assistant).
 * A window may go anywhere in the app window; only its header stays reachable (below the status bar, 44 px
 * of it on screen).
 */

/** Top edge a window header may go to: below the status bar. */
export function windowTopMin(): number {
  if (typeof document === "undefined") return 8;
  const px = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--status-top"));
  return Math.max(8, Number.isFinite(px) ? px : 0);
}

/** A window's top-left kept inside the app window, its header reachable. */
export function clampToWindow(x: number, y: number, w: number): { x: number; y: number } {
  const top = windowTopMin();
  return {
    x: Math.min(Math.max(8, x), Math.max(8, window.innerWidth - w - 8)),
    y: Math.min(Math.max(top, y), Math.max(top, window.innerHeight - 44)),
  };
}

/** A window's top-left so that all of it is in the app window: for where it opens, not for a drag. */
export function fitToWindow(x: number, y: number, w: number, h: number): { x: number; y: number } {
  const top = windowTopMin();
  return {
    x: Math.min(Math.max(8, x), Math.max(8, window.innerWidth - w - 8)),
    y: Math.min(Math.max(top, y), Math.max(top, window.innerHeight - h - 8)),
  };
}

/** What the header may not start a drag on. */
const NO_DRAG = "button, a, input, select, textarea, [data-no-drag]";

/**
 * Header handlers that move a window. `rect` gives its box when the drag starts; `onMove` gets the new
 * top-left, already kept inside the app window.
 */
export function useFloatDrag(opts: { enabled: boolean; rect: () => DOMRect | null | undefined; onMove: (x: number, y: number) => void; onEnd?: () => void }) {
  const hold = useRef<{ dx: number; dy: number; w: number } | null>(null);
  const end = () => {
    if (!hold.current) return;
    hold.current = null;
    opts.onEnd?.();
  };
  return {
    onPointerDown(ev: ReactPointerEvent<HTMLElement>) {
      if (!opts.enabled || ev.button !== 0 || (ev.target as HTMLElement).closest(NO_DRAG)) return;
      const box = opts.rect();
      if (!box) return;
      hold.current = { dx: ev.clientX - box.left, dy: ev.clientY - box.top, w: box.width };
      try {
        ev.currentTarget.setPointerCapture(ev.pointerId);
      } catch {
        /* ignore invalid pointer id */
      }
    },
    onPointerMove(ev: ReactPointerEvent<HTMLElement>) {
      const h = hold.current;
      if (!h) return;
      const at = clampToWindow(ev.clientX - h.dx, ev.clientY - h.dy, h.w);
      opts.onMove(at.x, at.y);
    },
    onPointerUp: end,
    onPointerCancel: end,
    onLostPointerCapture: end,
  };
}

/** Handlers for a resize grip: `onSize` gets the new width and height. */
export function useFloatResize(opts: { enabled: boolean; rect: () => DOMRect | null | undefined; onSize: (w: number, h: number) => void; onEnd?: () => void; min?: { w: number; h: number } }) {
  const hold = useRef<{ x: number; y: number; w: number; h: number } | null>(null);
  const min = opts.min ?? { w: 320, h: 200 };
  const end = () => {
    if (!hold.current) return;
    hold.current = null;
    opts.onEnd?.();
  };
  return {
    onPointerDown(ev: ReactPointerEvent<HTMLElement>) {
      if (!opts.enabled || ev.button !== 0) return;
      const box = opts.rect();
      if (!box) return;
      hold.current = { x: ev.clientX, y: ev.clientY, w: box.width, h: box.height };
      ev.currentTarget.setPointerCapture(ev.pointerId);
      ev.preventDefault();
    },
    onPointerMove(ev: ReactPointerEvent<HTMLElement>) {
      const h = hold.current;
      if (!h) return;
      opts.onSize(Math.max(min.w, h.w + ev.clientX - h.x), Math.max(min.h, h.h + ev.clientY - h.y));
    },
    onPointerUp: end,
    onPointerCancel: end,
    onLostPointerCapture: end,
  };
}

/** The resize grip in a window's corner. */
export function ResizeGrip(props: ReturnType<typeof useFloatResize>) {
  return (
    <span className="float-grip" {...props} aria-hidden="true">
      <svg viewBox="0 0 12 12" width="12" height="12"><path d="M11 5L5 11M11 8.5L8.5 11" /></svg>
    </span>
  );
}

/** The standard header buttons: open in separate window, full screen, close. A button whose handler is missing is left out. */
export function WindowButtons({ onDetach, full, onFull, onClose, className }: { onDetach?: () => void; full?: boolean; onFull?: () => void; onClose?: () => void; className?: string }) {
  const t = useT();
  return (
    <>
      {onDetach ? <DetachButton className={className} onDetach={onDetach} /> : null}
      {onFull ? (
        <button type="button" className={className} aria-pressed={!!full} aria-label={full ? t("Exit full screen") : t("Full screen")} title={full ? t("Exit full screen") : t("Full screen")} onClick={onFull}>
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
            {full ? (
              <path d="M2 6h4V2M14 6h-4V2M2 10h4v4M14 10h-4v4" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
            ) : (
              <path d="M6 2H2v4M10 2h4v4M6 14H2v-4M10 14h4v-4" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
            )}
          </svg>
        </button>
      ) : null}
      {onClose ? (
        <button type="button" className={className} aria-label={t("Close")} title={t("Close")} onClick={onClose}>
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
            <path d="M3.2 3.2 12.8 12.8M12.8 3.2 3.2 12.8" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
        </button>
      ) : null}
    </>
  );
}

/** Where a window goes when it opens in its own OS window: the panel kind, its payload, and the window title. */
export type FloatDetach = { kind: DetachKind; payload: unknown; title: string; key?: string };

/**
 * A floating window. `className` adds the window's own size class (see `.float-win` in styles.css).
 * `head` goes in the header after the title; `backdrop` dims the app and a click on it closes the window.
 * `detach` adds "Open in separate window"; once that window is open, this one closes.
 */
export function FloatWindow({
  title,
  head,
  children,
  onClose,
  className = "",
  detached = false,
  detach,
  backdrop = false,
}: {
  title: ReactNode;
  head?: ReactNode;
  children: ReactNode;
  onClose: () => void;
  className?: string;
  detached?: boolean;
  detach?: FloatDetach;
  backdrop?: boolean;
}) {
  const frame = useRef<HTMLDivElement>(null);
  const placed = useRef<{ left: string; top: string; width: string; height: string } | null>(null);
  const [full, setFull] = useState(false);

  function placeFull(on: boolean) {
    const node = frame.current;
    if (!node) return;
    if (on) {
      placed.current = { left: node.style.left, top: node.style.top, width: node.style.width, height: node.style.height };
      node.style.left = "";
      node.style.top = "";
      node.style.width = "";
      node.style.height = "";
      setFull(true);
      return;
    }
    const prev = placed.current;
    const top = windowTopMin();
    node.style.left = prev?.left ?? "";
    node.style.top = prev?.top ? `${Math.max(top, parseFloat(prev.top) || top)}px` : "";
    node.style.width = prev?.width ?? "";
    node.style.height = prev?.height ?? "";
    setFull(false);
  }

  useEffect(() => {
    if (detached) return;
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      if (full) placeFull(false);
      else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [full, onClose, detached]);

  const drag = useFloatDrag({
    enabled: !full && !detached,
    rect: () => frame.current?.getBoundingClientRect(),
    onMove: (x, y) => {
      const node = frame.current;
      if (!node) return;
      node.style.left = `${x}px`;
      node.style.top = `${y}px`;
    },
  });

  async function onDetach() {
    if (detach && (await detachWindow(detach.kind, detach.payload, { title: detach.title, key: detach.key }))) onClose();
  }

  const label = typeof title === "string" ? title : undefined;
  const view = (
    <div
      className={`float-win${className ? ` ${className}` : ""}${backdrop ? "" : " modeless"}${detached ? " detached" : full ? " full" : ""}`}
      ref={frame}
      role="dialog"
      aria-label={label}
      onClick={(ev) => ev.stopPropagation()}
    >
      <header {...drag}>
        <b>{title}</b>
        {head}
        <WindowButtons onDetach={detach && !detached ? () => void onDetach() : undefined} full={full} onFull={detached ? undefined : () => placeFull(!full)} onClose={onClose} />
      </header>
      {children}
    </div>
  );
  if (detached) return view;
  return createPortal(backdrop ? <div className="float-back" onClick={onClose} role="presentation">{view}</div> : view, document.body);
}
