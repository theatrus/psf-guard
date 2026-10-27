import { Database, ChevronRight } from 'lucide-react';
import type { DbEntry } from '../utils/tauri';

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

export function RigDatabaseNavigation({ database, onBack }: {
  database: DbEntry; onBack: () => void;
}) {
  return <nav className="settings-rig-database-breadcrumb" aria-label="Rig database location" data-database-id={database.id}>
    <button type="button" className="link-button" onClick={onBack}>Databases &amp; rigs</button>
    <ChevronRight size={14} aria-hidden="true" /><strong>{database.name}</strong>
  </nav>;
}
