import { Link } from 'react-router-dom';
import { Settings } from 'lucide-react';
import { useAllDatabases } from '../../hooks/useDatabases';
import { openSettings } from '../../utils/settingsIntent';

export default function DirectorRigs() {
  const databases = useAllDatabases();
  return <section aria-label="Rig databases">
    <h2>Rigs</h2>
    {databases.isPending && <p role="status">Loading rig databases...</p>}
    {databases.isError && <div role="alert"><p>{databases.error.message}</p><button type="button" onClick={() => void databases.refetch()}>Retry</button></div>}
    {databases.data?.length === 0 && <button type="button" onClick={() => openSettings('add')}>Add database</button>}
    <ul className="director-list">{databases.data?.map(db => <li key={db.id}>
      <div className="director-record">
        <div className="director-record-name"><strong>{db.name}</strong><code>{db.id}</code></div>
        <Link to={`/?${new URLSearchParams({ db: db.id, dbfilter: db.id })}`}>Overview</Link>
        <button type="button" title={`Configure ${db.name}`} aria-label={`Configure ${db.name}`} onClick={() => openSettings({ kind: 'director-links', dbId: db.id })}><Settings size={16} /></button>
      </div>
    </li>)}</ul>
  </section>;
}
