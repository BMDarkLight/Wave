/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { BiCheck, BiChevronDown } from "react-icons/bi";
import ContextMenu, { type ContextMenuAnchor } from "./ContextMenu";

/** The menu matches its button's width up to this; see `.wave-select-menu`. */
const MAX_WIDTH = 360;
/** The menu's padding and border on both sides, so the list fits inside. */
const MENU_INSET = 14;

export type SelectOption = {
  value: string;
  label: string;
  disabled?: boolean;
  /** Shown on the button when selected, but not offered in the list. */
  hidden?: boolean;
};

/**
 * A dropdown that opens Wave's own menu in place of the system picker, so it
 * looks like every other menu in the app. `placeholder` shows while `value`
 * matches no option, which also makes it work as an action menu (EQ presets).
 */
export default function Select({
  value,
  options,
  onChange,
  placeholder,
  id,
  className = "",
  "aria-label": ariaLabel,
  disabled = false,
}: {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  placeholder?: string;
  id?: string;
  className?: string;
  "aria-label"?: string;
  disabled?: boolean;
}) {
  const button = useRef<HTMLButtonElement | null>(null);
  const list = useRef<HTMLDivElement | null>(null);
  const [anchor, setAnchor] = useState<ContextMenuAnchor | null>(null);
  const [width, setWidth] = useState(0);

  const current = options.find((option) => option.value === value);
  const shown = options.filter((option) => !option.hidden);

  const open = () => {
    const node = button.current;
    if (!node || disabled) return;
    const rect = node.getBoundingClientRect();
    setWidth(rect.width);
    setAnchor({
      top: rect.bottom + 6,
      left: rect.left,
      flipAbove: rect.top - 6,
    });
  };

  // Focus the chosen item, or the first, once the menu is on the page.
  const isOpen = anchor !== null;
  useEffect(() => {
    if (!isOpen) return;
    const chosen = list.current?.querySelector<HTMLButtonElement>(
      'button[aria-checked="true"]:not(:disabled)',
    );
    const first = list.current?.querySelector<HTMLButtonElement>(
      "button:not(:disabled)",
    );
    (chosen ?? first)?.focus({ preventScroll: true });
  }, [isOpen]);

  const close = () => {
    setAnchor(null);
    button.current?.focus();
  };

  const choose = (next: string) => {
    close();
    if (next !== value) onChange(next);
  };

  const onListKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(
      list.current?.querySelectorAll<HTMLButtonElement>(
        "button:not(:disabled)",
      ) ?? [],
    );
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const move = (index: number) => {
      event.preventDefault();
      items[(index + items.length) % items.length]?.focus();
    };
    if (event.key === "Escape" || event.key === "Tab") {
      event.preventDefault();
      close();
    } else if (event.key === "ArrowDown") move(at + 1);
    else if (event.key === "ArrowUp") move(at - 1);
    else if (event.key === "Home") move(0);
    else if (event.key === "End") move(items.length - 1);
  };

  return (
    <>
      <button
        ref={button}
        id={id}
        type="button"
        className={`wave-select ${className}`.trim()}
        aria-haspopup="menu"
        aria-expanded={anchor !== null}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={() => (anchor ? close() : open())}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            open();
          }
        }}
      >
        <span
          className={current ? "wave-select-value" : "wave-select-placeholder"}
        >
          {current?.label ?? placeholder ?? ""}
        </span>
        <BiChevronDown className="wave-select-chevron" aria-hidden />
      </button>
      {anchor && (
        <ContextMenu
          anchor={anchor}
          onClose={close}
          className="track-context-menu wave-select-menu"
          bottomSafe={8}
        >
          <div
            ref={list}
            className="wave-select-list"
            style={{ minWidth: Math.min(width, MAX_WIDTH) - MENU_INSET }}
            onKeyDown={onListKey}
          >
            {shown.map((option) => {
              const selected = option.value === value;
              return (
                <button
                  key={option.value}
                  type="button"
                  role="menuitemradio"
                  aria-checked={selected}
                  disabled={option.disabled}
                  className={selected ? "selected" : undefined}
                  onClick={() => choose(option.value)}
                >
                  <BiCheck className="wave-select-check" aria-hidden />
                  <span>{option.label}</span>
                </button>
              );
            })}
          </div>
        </ContextMenu>
      )}
    </>
  );
}
