import { X } from 'lucide-react';
import './toolbar.css';

export interface ScopeChipProps {
  /** What kind of scope it is, said before its name: "Project". */
  caption: string;
  /** What it is scoped to: "M 44". */
  name: string;
  /** More about it after the name, such as "2 rigs". */
  detail?: string;
  /** Said by the ✕ button: what removing the scope shows instead. */
  removeLabel: string;
  onRemove: () => void;
}

/**
 * The scope a view is narrowed to, "Project: M 44 ✕", first in its toolbar
 * and in sight at every width. It reads as a filter in use; ✕ removes it.
 * A scope that spans several rigs names them in `detail`.
 */
export default function ScopeChip({ caption, name, detail, removeLabel, onRemove }: ScopeChipProps) {
  const said = [`${caption}: ${name}`, detail].filter(Boolean).join(' · ');
  return (
    <span className="toolbar-pill toolbar-scope-chip is-set" role="group" aria-label={`Scope: ${said}`}
      title={said} data-testid="scope-chip">
      <span className="toolbar-pill-label" aria-hidden="true">{caption}:</span>
      <span className="toolbar-pill-value" aria-hidden="true">{name}</span>
      {detail && <span className="toolbar-scope-detail" aria-hidden="true">· {detail}</span>}
      <button type="button" className="toolbar-scope-remove" aria-label={removeLabel} title={removeLabel}
        onClick={onRemove}>
        <X size={13} aria-hidden="true" />
      </button>
    </span>
  );
}
