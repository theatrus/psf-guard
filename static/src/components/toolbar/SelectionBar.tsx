import type { ReactNode } from 'react';
import { X } from 'lucide-react';
import './toolbar.css';

interface SelectionBarProps {
  /** How many images are selected. */
  count: number;
  /** The count at which the actions appear. The Images grid needs two: a
   *  plain click there selects one image only to move the cursor. */
  minimum?: number;
  /** How to select, as the count's tooltip. */
  hint: string;
  /** Said beside the count once active, such as the rule that chose it. */
  note?: ReactNode;
  /** The actions, most important first. */
  children: ReactNode;
  onClear: () => void;
  className?: string;
}

/**
 * The selection end of a toolbar. At rest it says how many images are
 * selected, with how to select more as its tooltip; once enough are, it
 * turns into a highlighted bar with the count, the actions and Clear. Both
 * states are the same height, so the toolbar keeps its height either way.
 */
export default function SelectionBar({
  count,
  minimum = 1,
  hint,
  note,
  children,
  onClear,
  className,
}: SelectionBarProps) {
  const active = count >= minimum;
  return (
    <div
      className={`selection-action-bar${active ? ' active' : ''}${className ? ` ${className}` : ''}`}
      role="group"
      aria-label="Selected image actions"
    >
      <span className="selection-count" title={hint}>{count} selected</span>
      {active && (
        <>
          {note && <span className="selection-note">{note}</span>}
          <div className="selection-actions">{children}</div>
          <button
            type="button"
            className="tool-button tool-button-quiet selection-clear"
            title="Clear the selection"
            onClick={onClear}
          >
            <X size={14} aria-hidden="true" />
            <span className="tool-button-label">Clear</span>
          </button>
        </>
      )}
    </div>
  );
}
