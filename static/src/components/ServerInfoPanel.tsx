import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useAllDatabases } from '../hooks/useDatabases';
import Dialog from './Dialog';
import './ServerInfoPanel.css';

export default function ServerInfoPanel({ onClose }: { onClose: () => void }) {
  const info = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });
  const databases = useAllDatabases();

  return (
    <Dialog open title="Server information" onClose={onClose} className="server-info-panel"
      footer={<button type="button" className="action-button" onClick={onClose}>Done</button>}>
      <section className="server-info-content" aria-label="Server details" tabIndex={0}>
        {info.isPending && <p role="status">Loading server information...</p>}
        {info.isError && <p role="alert">
          Could not load server information.{' '}
          <button type="button" onClick={() => void info.refetch()}>Retry server information</button>
        </p>}
        {info.data && <dl className="server-info-fields">
          <dt>Version</dt><dd>{info.data.version}</dd>
          <dt>Cache folder</dt><dd><code className="server-info-path">{info.data.cache_directory}</code></dd>
        </dl>}

        <section className="server-info-databases" aria-label="Configured databases">
          <h3>Databases{databases.data ? ` (${databases.data.length})` : ''}</h3>
          {databases.isPending && <p role="status">Loading databases...</p>}
          {databases.isError && <p role="alert">
            Could not load configured databases.{' '}
            <button type="button" onClick={() => void databases.refetch()}>Retry databases</button>
          </p>}
          {databases.data?.length === 0 && <p>No databases configured.</p>}
          {!!databases.data?.length && <ul className="server-info-database-list">
            {databases.data.map(db => <li key={db.id} className="server-info-database">
              <h4>{db.name}</h4>
              <dl className="server-info-fields">
                <dt>Database ID</dt><dd><code className="server-info-path">{db.id}</code></dd>
                <dt>Database file</dt><dd><code className="server-info-path">{db.database_path}</code></dd>
                <dt>Image folders</dt><dd>{db.image_directories.length ?
                  <ul className="server-info-folder-list">{db.image_directories.map(folder =>
                    <li key={folder}><code className="server-info-path">{folder}</code></li>
                  )}</ul> : 'No image folders configured.'}</dd>
              </dl>
            </li>)}
          </ul>}
        </section>
      </section>
    </Dialog>
  );
}
