import { listFilterLabel } from '../utils/listFilter';
import ToolbarPopover from './toolbar/ToolbarPopover';

interface MultiSelectMenuProps {
  id: string;
  /** The caption over the button, such as "Filter". */
  label: string;
  options: ReadonlyArray<{ value: string; label: string }>;
  /** The values kept; none keeps them all. */
  chosen: readonly string[];
  onChange: (values: string[]) => void;
  /** Ticking every option is the same as All, so the choice clears. */
  everyIsAll?: boolean;
  /** With nothing chosen every box shows ticked, and unticking one keeps
   *  the rest: Status reads "show these grades" rather than "keep only". */
  tickAllWhenEmpty?: boolean;
  title?: string;
}

/**
 * The one multi-select of the toolbars: a pill that says what is kept,
 * "Filter: Ha, OIII", and opens a list of boxes with All at the top to
 * clear them.
 */
export default function MultiSelectMenu({
  id,
  label,
  options,
  chosen,
  onChange,
  everyIsAll = false,
  tickAllWhenEmpty = false,
  title,
}: MultiSelectMenuProps) {
  const caption = label.replace(/:$/, '');
  const labelOf = (value: string) => options.find((option) => option.value === value)?.label ?? value;
  const ticked = chosen.length === 0 && tickAllWhenEmpty ? options.map((option) => option.value) : chosen;
  const toggle = (value: string) => {
    const kept = ticked.includes(value) ? ticked.filter((entry) => entry !== value) : [...ticked, value];
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
    <ToolbarPopover
      id={id}
      label={caption}
      value={listFilterLabel(chosen, labelOf)}
      active={chosen.length > 0}
      title={chosen.length > 0 ? chosen.map(labelOf).join(', ') : title}
    >
      <label className="multi-select-choice">
        <input type="checkbox" checked={chosen.length === 0} onChange={() => onChange([])} />
        All
      </label>
      {options.map((option) => (
        <label key={option.value} className="multi-select-choice">
          <input type="checkbox" checked={ticked.includes(option.value)} onChange={() => toggle(option.value)} />
          {option.label}
        </label>
      ))}
    </ToolbarPopover>
  );
}
