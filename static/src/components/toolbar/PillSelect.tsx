import { useLayoutEffect, useRef, useState, type SelectHTMLAttributes } from 'react';
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
 *
 * The select lies unseen over the whole pill, so a click on the name or
 * the chevron opens its list as a click on the value does: a label only
 * focuses a select, and WebKit has no `showPicker` for one. The pill shows
 * the chosen option's text.
 */
export default function PillSelect({
  label,
  active = false,
  pillClassName,
  pillTitle,
  onChange,
  ...select
}: PillSelectProps) {
  const ref = useRef<HTMLSelectElement>(null);
  const [shown, setShown] = useState('');
  // The value or the options' text may change from outside.
  const { value, defaultValue, children } = select;
  useLayoutEffect(() => {
    setShown(ref.current?.selectedOptions[0]?.text ?? '');
  }, [value, defaultValue, children]);
  return (
    <label
      className={`toolbar-pill toolbar-pill-select${active ? ' is-set' : ''}${pillClassName ? ` ${pillClassName}` : ''}`}
      title={pillTitle}
    >
      <span className="toolbar-pill-label">{label}:</span>
      <span className="toolbar-pill-value" aria-hidden="true">{shown}</span>
      <select
        ref={ref}
        aria-label={label}
        {...select}
        onChange={(event) => {
          // A controlled pill shows the value it is given back, not the
          // choice: a parent may refuse it.
          if (value === undefined) setShown(event.currentTarget.selectedOptions[0]?.text ?? '');
          onChange?.(event);
        }}
      />
    </label>
  );
}
