import type { ReactNode } from 'react';

/** The same expandable override row for a calibration rig or a database. */
export default function SettingsOverrideItem({
  name, meta, count, expanded, editorId, onToggle, children,
}: {
  name: string;
  meta?: string;
  count: number;
  expanded: boolean;
  editorId: string;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <li className="calibration-rig-item">
      <button
        type="button"
        className="calibration-rig-toggle"
        aria-expanded={expanded}
        aria-controls={editorId}
        onClick={onToggle}
      >
        <span className="calibration-rig-name">{name}</span>
        {meta && <span className="calibration-rig-meta">{meta}</span>}
        <span className={`calibration-rig-count${count > 0 ? ' has-own-values' : ''}`}>
          {count > 0 ? `${count} ${count === 1 ? 'override' : 'overrides'}` : 'Uses global settings'}
        </span>
      </button>
      {children}
    </li>
  );
}
