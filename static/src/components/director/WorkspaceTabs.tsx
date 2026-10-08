import type { KeyboardEvent, ReactNode } from 'react';

/** The workspace tab pattern, with one focus stop and keyboard navigation. */
export default function WorkspaceTabs<T extends string>({ id, label, tabs, value, onChange, orientation = 'horizontal', className = '' }: {
  id: string; label: string; tabs: readonly { id: T; label: string; icon?: ReactNode }[]; value: T; onChange: (value: T) => void;
  orientation?: 'horizontal' | 'vertical'; className?: string;
}) {
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const index = tabs.findIndex(tab => tab.id === value);
    const next = event.key === (orientation === 'vertical' ? 'ArrowDown' : 'ArrowRight') ? (index + 1) % tabs.length
      : event.key === (orientation === 'vertical' ? 'ArrowUp' : 'ArrowLeft') ? (index + tabs.length - 1) % tabs.length
        : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : null;
    if (next === null) return;
    event.preventDefault();
    onChange(tabs[next].id);
    document.getElementById(`${id}-tab-${tabs[next].id}`)?.focus();
  };
  return <div className={`workspace-tabs ${className}`.trim()} role="tablist" aria-label={label} aria-orientation={orientation} onKeyDown={onKeyDown}>
    {tabs.map(tab => <button key={tab.id} type="button" role="tab" id={`${id}-tab-${tab.id}`} aria-controls={`${id}-panel-${tab.id}`}
      aria-selected={tab.id === value} tabIndex={tab.id === value ? 0 : -1} className={`workspace-tab${tab.id === value ? ' active' : ''}`} onClick={() => onChange(tab.id)}>{tab.icon}{tab.label}</button>)}
  </div>;
}
