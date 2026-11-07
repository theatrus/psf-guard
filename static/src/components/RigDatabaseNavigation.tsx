import { Database, ChevronRight, Trash2 } from 'lucide-react';
import type { DbEntry } from '../utils/tauri';

/** The registered databases. `onRemove` is absent where the server does not
 *  allow database changes; `managementHint` then says how to allow them. */
export function RigDatabaseList({ databases, onOpen, onRemove, managementHint = false }: {
  databases: readonly DbEntry[];
  onOpen: (entry: DbEntry) => void;
  onRemove?: (entry: DbEntry) => void;
  managementHint?: boolean;
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
        {onRemove && <button type="button" className="db-action-button is-danger" aria-label={`Remove ${entry.name}`}
          title={`Remove ${entry.name} and its rig from PSF Guard; its files stay on disk`} onClick={() => onRemove(entry)}>
          <Trash2 size={15} aria-hidden="true" />Remove
        </button>}
      </li>)}
    </ul>
    {managementHint && <p className="settings-note">To add or remove databases here, start the server with <code>--allow-database-management</code>.</p>}
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
