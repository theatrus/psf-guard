import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useAllDatabases } from '../hooks/useDatabases';

/** What the server runs and where it keeps things, opened from the
 *  header menu. */
export default function ServerInfoPanel({ onClose }: { onClose: () => void }) {
  const { data: serverInfo, isLoading, error } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000, // Cache for 5 minutes
  });

  const { data: databases } = useAllDatabases();

  return (
    <div className="info-panel-overlay" onClick={onClose}>
      <div className="info-panel" onClick={e => e.stopPropagation()}>
        <div className="info-panel-header">
          <h3>Server Information</h3>
          <button className="close-button" onClick={onClose} aria-label="Close server information">×</button>
        </div>

        <div className="info-panel-content">
          {isLoading && <div className="loading">Loading...</div>}
          {error && <div className="error">Failed to load server info</div>}
          {serverInfo && (
            <dl>
              <dt>Version:</dt>
              <dd>{serverInfo.version}</dd>

              <dt>Cache Directory:</dt>
              <dd className="path" title={serverInfo.cache_directory}>
                {serverInfo.cache_directory}
              </dd>

              <dt>Databases ({databases?.length ?? 0}):</dt>
              <dd>
                {databases?.length ? (
                  <ul className="db-list">
                    {databases.map((db) => (
                      <li key={db.id}>
                        <strong>{db.name}</strong> <code>({db.id})</code>
                        <div className="path" title={db.database_path}>
                          {db.database_path}
                        </div>
                        {db.image_directories.length > 0 && (
                          <div className="image-dirs">
                            {db.image_directories.join(', ')}
                          </div>
                        )}
                      </li>
                    ))}
                  </ul>
                ) : (
                  <em>None configured</em>
                )}
              </dd>
            </dl>
          )}
        </div>
      </div>
    </div>
  );
}