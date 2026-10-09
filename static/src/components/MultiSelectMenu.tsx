import { useEffect, useRef, useState } from 'react';
import { listFilterLabel } from '../utils/listFilter';

interface MultiSelectMenuProps {
  id: string;
  /** The caption beside the button, such as "Filter:". */
  label: string;
  options: ReadonlyArray<{ value: string; label: string }>;
  /** The values kept; none keeps them all. */
  chosen: readonly string[];
  onChange: (values: string[]) => void;
  /** Ticking every option is the same as All, so the choice clears. */
  everyIsAll?: boolean;
  title?: string;
}

/**
 * A filter that keeps any of several values: a button that says what is
 * kept and opens a list of boxes, with All at the top to clear them. The
 * list closes on a click outside it or on Escape.
 */
export default function MultiSelectMenu({
  id,
  label,
  options,
  chosen,
  onChange,
  everyIsAll = false,
  title,
}: MultiSelectMenuProps) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.stopPropagation();
      setOpen(false);
      button.current?.focus();
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape, true);
    return () => {
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('keydown', escape, true);
    };
  }, [open]);

  const labelOf = (value: string) => options.find((option) => option.value === value)?.label ?? value;
  const toggle = (value: string) => {
    const kept = chosen.includes(value) ? chosen.filter((entry) => entry !== value) : [...chosen, value];
    // In the list's own order, so the button and the URL read the same way
    // whatever order the boxes were ticked in.
    const ordered = [
      ...options.map((option) => option.value).filter((entry) => kept.includes(entry)),
      ...kept.filter((entry) => !options.some((option) => option.value === entry)),
    ];
    onChange(everyIsAll && options.length > 0 && ordered.length >= options.length
      && options.every((option) => ordered.includes(option.value)) ? [] : ordered);
  };

  return (
    <div className="filter-input-group multi-select" ref={root}>
      <span id={`${id}-label`}>{label}</span>
      <button
        ref={button}
        type="button"
        id={id}
        className="multi-select-button"
        aria-haspopup="true"
        aria-expanded={open}
        aria-labelledby={`${id}-label ${id}`}
        title={title}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="multi-select-value">{listFilterLabel(chosen, labelOf)}</span>
        <span aria-hidden="true">▾</span>
      </button>
      {open && (
        <div className="multi-select-menu" role="group" aria-label={label.replace(/:$/, '')}>
          <label className="multi-select-choice">
            <input type="checkbox" checked={chosen.length === 0} onChange={() => onChange([])} />
            All
          </label>
          {options.map((option) => (
            <label key={option.value} className="multi-select-choice">
              <input type="checkbox" checked={chosen.includes(option.value)} onChange={() => toggle(option.value)} />
              {option.label}
            </label>
          ))}
        </div>
      )}
    </div>
  );
}
