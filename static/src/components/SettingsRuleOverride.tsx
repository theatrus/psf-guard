import type { ReactNode } from 'react';

/** An inherited value or its editor, with the shared override control. */
export default function SettingsRuleOverride({
  label, isOwn, inheritedValue, source, target, disabled, onToggle, children,
}: {
  label: string;
  isOwn: boolean;
  inheritedValue: ReactNode;
  source: string;
  target: 'rig' | 'database';
  disabled?: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  return (
    <>
      {isOwn ? children : (
        <p className="calibration-rule-inherited">
          {inheritedValue}<small> · {source}</small>
        </p>
      )}
      <button
        type="button"
        className="calibration-rule-own"
        aria-pressed={isOwn}
        aria-label={`${label}: override global setting`}
        title={isOwn ? 'Use global setting' : `Set a value for this ${target}`}
        disabled={disabled}
        onClick={onToggle}
      >
        Override global setting
      </button>
    </>
  );
}
