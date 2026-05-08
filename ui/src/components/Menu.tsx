import {
  cloneElement,
  isValidElement,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  onClick: () => void | Promise<void>;
  disabled?: boolean;
  danger?: boolean;
  separatorBefore?: boolean;
}

interface Props {
  /** A clickable trigger element. Receives onClick injected. */
  trigger: ReactElement;
  items: MenuItem[];
  align?: "left" | "right";
}

/* Popover menu rendered into a portal at the document body so it
 * escapes any ancestor `overflow: auto/hidden` clip — the table /
 * sidebar / drawer all create such contexts and a plain
 * `position: absolute` popover gets cut off near the edges. We anchor
 * to the trigger's bounding rect on open and re-anchor on scroll/
 * resize so the menu tracks the trigger if anything below moves. */
export function Menu({ trigger, items, align = "right" }: Props) {
  const [open, setOpen] = useState(false);
  const [coords, setCoords] = useState<{ top: number; left: number; width: number } | null>(null);
  const triggerRef = useRef<HTMLElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);

  // Position the popover next to the trigger. Right-aligned by default
  // (the trigger is usually on the right edge of a row); left-align is
  // available for cases where there isn't room.
  const position = () => {
    const node = triggerRef.current;
    if (!node) return;
    const r = node.getBoundingClientRect();
    const menuWidth = menuRef.current?.offsetWidth ?? 220;
    const top = r.bottom + 6;
    let left = align === "right" ? r.right - menuWidth : r.left;
    // Clamp inside viewport.
    const margin = 8;
    if (left + menuWidth > window.innerWidth - margin) {
      left = window.innerWidth - margin - menuWidth;
    }
    if (left < margin) left = margin;
    setCoords({ top, left, width: menuWidth });
  };

  useLayoutEffect(() => {
    if (!open) return;
    position();
    // Re-anchor on scroll/resize so the menu tracks the trigger.
    const handler = () => position();
    window.addEventListener("scroll", handler, true);
    window.addEventListener("resize", handler);
    return () => {
      window.removeEventListener("scroll", handler, true);
      window.removeEventListener("resize", handler);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!open) return;
    function down(e: MouseEvent) {
      const t = e.target as Node;
      if (
        triggerRef.current?.contains(t) ||
        menuRef.current?.contains(t)
      ) {
        return;
      }
      setOpen(false);
    }
    function key(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    window.addEventListener("mousedown", down);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("mousedown", down);
      window.removeEventListener("keydown", key);
    };
  }, [open]);

  const triggerEl = isValidElement<{
    onClick?: (e: React.MouseEvent) => void;
    ref?: (n: HTMLElement | null) => void;
  }>(trigger)
    ? cloneElement(trigger, {
        ref: (n: HTMLElement | null) => {
          triggerRef.current = n;
        },
        onClick: (e: React.MouseEvent) => {
          e.stopPropagation();
          setOpen((v) => !v);
        },
      })
    : trigger;

  return (
    <>
      {triggerEl}
      {open &&
        createPortal(
          <div
            ref={menuRef}
            className="popover menu"
            role="menu"
            style={{
              position: "fixed",
              top: coords?.top ?? -9999,
              left: coords?.left ?? -9999,
              right: "auto",
              minWidth: 220,
            }}
            onClick={(e) => e.stopPropagation()}
          >
            {items.map((it, i) => (
              <span key={i}>
                {it.separatorBefore && <div className="menu-divider" />}
                <button
                  role="menuitem"
                  disabled={it.disabled}
                  style={it.danger ? { color: "var(--danger)" } : undefined}
                  onClick={async () => {
                    setOpen(false);
                    await it.onClick();
                  }}
                >
                  {it.icon}
                  <span>{it.label}</span>
                </button>
              </span>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
