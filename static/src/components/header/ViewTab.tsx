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
 * its name. Any other shows its name while hovered or focused, drawn over
 * the tabs beside it rather than pushing them along, so nothing moves under
 * the pointer and the header's row fit does not change.
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
      {current ? (
        <span className="view-tab-name">{label}</span>
      ) : (
        <span className="view-tab-flyout" aria-hidden="true">{label}</span>
      )}
    </button>
  );
}
