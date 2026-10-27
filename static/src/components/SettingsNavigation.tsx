import { ChevronRight } from 'lucide-react';
import { useLayoutEffect, useRef, type KeyboardEvent } from 'react';
import { settingsPage, type SettingsSection } from './settingsSections';

export interface NavigationGroup {
  id: string; label: string; pages: readonly { id: string; label: string; mobileLabel?: string; depth?: number; expanded?: boolean; context?: string }[];
}

export default function SettingsNavigation({ groups, current, onChange, loading = false }: {
  groups: readonly NavigationGroup[];
  current: string;
  loading?: boolean;
  onChange: (section: string) => void;
}) {
  const pages = groups.flatMap(group => group.pages);
  const navigationKey = pages.map(page => page.id).join(',');
  const sidebar = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const nav = sidebar.current;
    if (!nav) return;
    const showCurrent = () => {
      const selected = nav.querySelector<HTMLButtonElement>("[aria-selected='true']");
      if (!selected) return;
      const bounds = nav.getBoundingClientRect();
      const item = selected.getBoundingClientRect();
      if (item.top < bounds.top) nav.scrollTop += item.top - bounds.top;
      else if (item.bottom > bounds.bottom) nav.scrollTop += item.bottom - bounds.bottom;
    };
    showCurrent();
    if (typeof ResizeObserver === 'undefined') return;
    const resize = new ResizeObserver(showCurrent);
    resize.observe(nav);
    return () => resize.disconnect();
  }, [current, navigationKey]);
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const index = pages.findIndex(page => page.id === current);
    const next = event.key === 'ArrowDown' || event.key === 'ArrowRight' ? (index + 1) % pages.length
      : event.key === 'ArrowUp' || event.key === 'ArrowLeft' ? (index + pages.length - 1) % pages.length
        : event.key === 'Home' ? 0 : event.key === 'End' ? pages.length - 1 : null;
    if (next === null) return;
    event.preventDefault();
    onChange(pages[next].id);
    document.getElementById(`settings-tab-${pages[next].id}`)?.focus();
  };

  return <>
    <div className="settings-sidebar" ref={sidebar} role="tablist" aria-label="Settings sections"
      aria-orientation="vertical" onKeyDown={onKeyDown}>
      {groups.map(group => <div className="settings-nav-group" key={group.id} role="presentation">
        <span className="settings-nav-group-label" id={`settings-group-${group.id}`} aria-hidden="true">{group.label}</span>
        {group.pages.map(page => <button key={page.id} type="button" role="tab"
          id={`settings-tab-${page.id}`} aria-selected={page.id === current}
          aria-controls={`settings-panel-${page.id}`} aria-describedby={`settings-group-${group.id}`}
          tabIndex={page.id === current ? 0 : -1} className="settings-tab" data-depth={page.depth} aria-expanded={page.expanded}
          onClick={() => onChange(page.id)}>{page.expanded !== undefined && <ChevronRight size={13} aria-hidden="true" />}{page.label}</button>)}
      </div>)}
    </div>
    <div className="settings-mobile-nav">
      <label htmlFor="settings-section-picker">Settings section</label>
      <select id="settings-section-picker" value={current} onChange={event => onChange(event.target.value)}
        aria-controls={`settings-panel-${current}`}>
        {!pages.some(page => page.id === current) && <option value={current} disabled>
          {current.startsWith('database:') ? (loading ? 'Loading selected database…' : 'Selected database unavailable') : `${settingsPage(current as SettingsSection)?.label ?? 'Settings'} — Loading…`}
        </option>}
        {groups.map(group => <optgroup key={group.id} label={group.label}>
          {group.pages.map(page => <option key={page.id} value={page.id}>{page.mobileLabel ?? page.label}</option>)}
        </optgroup>)}
      </select>
    </div>
  </>;
}
