import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ChevronDown } from 'lucide-react';
import './toolbar.css';

interface ToolbarPopoverProps {
  id: string;
  /** The caption over the button, such as "Filter". */
  label: string;
  /** What the field keeps, in a few words, such as "All" or "Ha, OIII". */
  value: string;
  /** The field narrows the view; its button stands out. */
  active?: boolean;
  /** Said on hover. */
  title?: string;
  className?: string;
  children: ReactNode;
}

/**
 * A toolbar field that opens a small panel under its button: the
 * multi-selects and the date range. The panel closes on a click outside
 * it or on Escape, which hands focus back to the button.
 */
export default function ToolbarPopover({
  id,
  label,
  value,
  active = false,
  title,
  className,
  children,
}: ToolbarPopoverProps) {
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

  return (
    <div className={`toolbar-field multi-select${className ? ` ${className}` : ''}`} ref={root}>
      <span className="toolbar-caption" aria-hidden="true">{label}</span>
      <button
        ref={button}
        type="button"
        id={id}
        className={`multi-select-button${active ? ' is-set' : ''}`}
        aria-haspopup="true"
        aria-expanded={open}
        aria-label={`${label}: ${value}`}
        title={title}
        onClick={() => setOpen((shown) => !shown)}
      >
        <span className="multi-select-value">{value}</span>
        <ChevronDown size={14} aria-hidden="true" />
      </button>
      {open && (
        <div className="multi-select-menu" role="group" aria-label={label}>
          {children}
        </div>
      )}
    </div>
  );
}
