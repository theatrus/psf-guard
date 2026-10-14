import type { SelectHTMLAttributes } from 'react';
import './toolbar.css';

interface PillSelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  /** The field's name, said in the pill before its value: "Type". */
  label: string;
  /** The field narrows the view; its pill stands out. */
  active?: boolean;
  /** Classes for the pill around the select. */
  pillClassName?: string;
  /** Said on hover over the pill. */
  pillTitle?: string;
}

/**
 * A filter pill around a native select, "Type: Flat ▾", for a field that
 * keeps one value. The select keeps its own list and keyboard; its name is
 * the label unless an aria-label says more.
 */
export default function PillSelect({
  label,
  active = false,
  pillClassName,
  pillTitle,
  ...select
}: PillSelectProps) {
  return (
    <label
      className={`toolbar-pill toolbar-pill-select${active ? ' is-set' : ''}${pillClassName ? ` ${pillClassName}` : ''}`}
      title={pillTitle}
    >
      <span className="toolbar-pill-label">{label}:</span>
      <select aria-label={label} {...select} />
    </label>
  );
}
