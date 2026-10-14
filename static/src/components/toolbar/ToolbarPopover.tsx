import type { ReactNode } from 'react';
import { ChevronDown } from 'lucide-react';
import { usePopover } from './usePopover';
import './toolbar.css';

interface ToolbarPopoverProps {
  id: string;
  /** The field's name, said in the pill before its value: "Filter". */
  label: string;
  /** What the field keeps, in a few words, such as "All" or "Ha, OIII". */
  value: string;
  /** The field narrows the view; its pill stands out. */
  active?: boolean;
  /** Said on hover. */
  title?: string;
  className?: string;
  children: ReactNode;
}

/**
 * A filter pill, "Filter: Ha ▾", that opens a small panel under it: the
 * multi-selects and the date and temperature ranges. The panel stays
 * mounted while closed, so a number typed in it is still committed when a
 * click elsewhere closes it.
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
  const { open, setOpen, rootRef, buttonRef } = usePopover();
  return (
    <div className={`toolbar-popover${className ? ` ${className}` : ''}`} ref={rootRef}>
      <button
        ref={buttonRef}
        type="button"
        id={id}
        className={`toolbar-pill${active ? ' is-set' : ''}`}
        aria-haspopup="true"
        aria-expanded={open}
        aria-controls={`${id}-panel`}
        aria-label={`${label}: ${value}`}
        title={title}
        onClick={() => setOpen((shown) => !shown)}
      >
        <span className="toolbar-pill-label">{label}:</span>
        <span className="toolbar-pill-value">{value}</span>
        <ChevronDown size={13} aria-hidden="true" />
      </button>
      <div id={`${id}-panel`} className="toolbar-menu" role="group" aria-label={label} hidden={!open}>
        {children}
      </div>
    </div>
  );
}
