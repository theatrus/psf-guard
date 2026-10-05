import { useState, type ChangeEvent, type InputHTMLAttributes } from 'react';

/** A complete number as typed: not empty, not a lone sign or a trailing
 *  point, which `Number` would read as 0 or drop. */
const complete = (text: string) => /^\s*[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?\s*$/.test(text) && Number.isFinite(Number(text));

/** A number field that lets the text be edited freely. A controlled
 *  `type="number"` input fed the parsed value turns an emptied field into 0
 *  at once, so a new number cannot be typed over the old one. This keeps
 *  what is typed, passes a change on only once the text is a complete
 *  number, and shows the value again when the field loses focus. */
export default function NumberInput({ value, onChange, onBlur, ...rest }: Omit<InputHTMLAttributes<HTMLInputElement>, 'value' | 'type'> & {
  /** The committed value; empty when there is none yet. */
  value: number | '';
}) {
  // What was typed, and the value it stands for. A value that changes
  // from outside (a unit conversion, a clamp) replaces the text.
  const [draft, setDraft] = useState<{ text: string; value: number | '' } | null>(null);
  const shown = draft && draft.value === value ? draft.text : String(value);
  return <input
    {...rest}
    type="number"
    value={shown}
    onChange={(event: ChangeEvent<HTMLInputElement>) => {
      const text = event.target.value;
      if (complete(text)) {
        setDraft({ text, value: Number(text) });
        onChange?.(event);
      } else {
        setDraft({ text, value });
      }
    }}
    onBlur={event => { setDraft(null); onBlur?.(event); }}
  />;
}
