import type { ComponentType } from 'react';

interface ViewTabProps {
  label: string;
  icon: ComponentType<{ size?: number; 'aria-hidden'?: boolean | 'true' }>;
  current: boolean;
  disabled?: boolean;
  /** Why the tab is off, when it is. */
  title?: string;
  onClick: () => void;
}

/**
 * One view in the header, shown by its icon. The current view also shows
 * its name. Any other slides its name out of the button while hovered or
 * focused, and the current view folds its name away meanwhile, still
 * highlighted, so the row keeps about the same width.
 */
export default function ViewTab({ label, icon: Icon, current, disabled, title, onClick }: ViewTabProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="header-button view-tab"
      aria-current={current ? 'page' : undefined}
      aria-label={label}
      disabled={disabled}
      title={title}
    >
      <Icon size={16} aria-hidden="true" />
      <span className="view-tab-name" aria-hidden="true">{label}</span>
    </button>
  );
}
