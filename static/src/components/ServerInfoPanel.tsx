import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';

export default function ServerInfoPanel() {
  const [isOpen, setIsOpen] = useState(false);
  
  const { data: serverInfo, isLoading, error } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000, // Cache for 5 minutes
  });

  return (
    <>
      {/* Info button in top-right corner */}
      <button 
        className="info-button"
        onClick={() => setIsOpen(!isOpen)}
        title="Server Information"
      >
        <svg width="20" height="20" viewBox="0 0 20 20" fill="none" xmlns="http://www.w3.org/2000/svg">
          <circle cx="10" cy="10" r="9" stroke="currentColor" strokeWidth="2"/>
          <path d="M10 9V14M10 6V6.01" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
        </svg>
      </button>

      {/* Info panel overlay */}
      {isOpen && (
        <div className="info-panel-overlay" onClick={() => setIsOpen(false)}>
          <div className="info-panel" onClick={e => e.stopPropagation()}>
            <div className="info-panel-header">
              <h3>Server Information</h3>
              <button className="close-button" onClick={() => setIsOpen(false)}>×</button>
            </div>
            
            <div className="info-panel-content">
              {isLoading && <div className="loading">Loading...</div>}
              {error && <div className="error">Failed to load server info</div>}
              {serverInfo && (
                <dl>
                  <dt>Version:</dt>
                  <dd>{serverInfo.version}</dd>
                  
                  <dt>Database:</dt>
                  <dd className="path" title={serverInfo.database_path}>
                    {serverInfo.database_path}
                  </dd>
                  
                  <dt>Image Directory:</dt>
                  <dd className="path" title={serverInfo.image_directory}>
                    {serverInfo.image_directory}
                  </dd>
                  
                  <dt>Cache Directory:</dt>
                  <dd className="path" title={serverInfo.cache_directory}>
                    {serverInfo.cache_directory}
                  </dd>
                </dl>
              )}
            </div>
          </div>
        </div>
      )}
    </>
  );
}