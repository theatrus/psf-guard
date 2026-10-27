import { Database, ChevronRight } from 'lucide-react';
import type { DbEntry } from '../utils/tauri';
import { RIG_DATABASE_PAGES, type SettingsSection } from './settingsSections';

export function RigDatabaseList({ databases, onOpen }: {
  databases: readonly DbEntry[];
  onOpen: (entry: DbEntry) => void;
}) {
  return <section className="settings-section" aria-label="Rig databases">
    <h3>Databases &amp; rigs</h3>
    <p>Open a rig database to configure its equipment, files or sync connections.</p>
    <ul className="settings-rig-databases">
      {databases.map(entry => <li key={entry.id}>
        <button type="button" className="settings-rig-database-open" aria-label={`Open ${entry.name}`}
          onClick={() => onOpen(entry)}>
          <Database size={18} aria-hidden="true" />
          <span><strong>{entry.name}</strong><small>{entry.db_path}</small></span>
          <ChevronRight size={18} aria-hidden="true" />
        </button>
      </li>)}
    </ul>
  </section>;
}

export function RigDatabaseNavigation({ database, current, planningEnabled, canSync, onBack, onChange }: {
  database: DbEntry;
  current: SettingsSection;
  planningEnabled: boolean;
  canSync: boolean;
  onBack: () => void;
  onChange: (section: 'rigs' | 'databases' | 'sync') => void;
}) {
  return <div className="settings-rig-database-context">
    <nav className="settings-rig-database-breadcrumb" aria-label="Rig database location">
      <button type="button" className="link-button" onClick={onBack}>Databases &amp; rigs</button>
      <ChevronRight size={14} aria-hidden="true" /><strong>{database.name}</strong>
    </nav>
    <nav className="settings-rig-database-pages" aria-label={`Settings for ${database.name}`}>
      {RIG_DATABASE_PAGES.filter(page => (page.id !== 'rigs' || planningEnabled) && (page.id !== 'sync' || canSync))
        .map(page => <button key={page.id} type="button" aria-current={current === page.id ? 'page' : undefined}
          onClick={() => onChange(page.id)}>{page.label}</button>)}
    </nav>
  </div>;
}
