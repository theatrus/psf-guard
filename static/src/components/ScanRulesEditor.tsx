import { useState } from 'react';
import type { ScanRules } from '../api/types';
import ScanRulesFields from './ScanRulesFields';

/** Saved filters stay active while their optional editor is collapsed. */
export default function ScanRulesEditor({ value, onChange, status, open, onOpenChange }: {
  value: ScanRules;
  onChange: (next: ScanRules) => void;
  status: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  return (
    <details
      className="calibration-settings-group calibration-rig-group"
      open={open ?? expanded}
      onToggle={event => {
        const next = event.currentTarget.open;
        if (open === undefined) setExpanded(next);
        onOpenChange?.(next);
      }}
    >
      <summary>Scan rules · {status}</summary>
      <ScanRulesFields value={value} onChange={onChange} showLegend={false} />
    </details>
  );
}
