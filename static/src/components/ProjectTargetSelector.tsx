import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';

interface ProjectTargetSelectorProps {
  selectedProjectId: number | null;
  selectedTargetId: number | null;
  onProjectChange: (projectId: number | null) => void;
  onTargetChange: (targetId: number | null) => void;
}

export default function ProjectTargetSelector({
  selectedProjectId,
  selectedTargetId,
  onProjectChange,
  onTargetChange,
}: ProjectTargetSelectorProps) {
  const queryClient = useQueryClient();
  const [lastRefreshStatus, setLastRefreshStatus] = useState<string | null>(null);

  // Refresh cache mutation
  const refreshCacheMutation = useMutation({
    mutationFn: apiClient.refreshFileCache,
    onSuccess: (data) => {
      // Invalidate and refetch projects and targets
      queryClient.invalidateQueries({ queryKey: ['projects'] });
      queryClient.invalidateQueries({ queryKey: ['targets'] });
      setLastRefreshStatus(`Checked ${data.images_checked} projects: ${data.files_found} with files, ${data.files_missing} without (${data.check_time_ms}ms)`);
      
      // Clear status after 5 seconds
      setTimeout(() => setLastRefreshStatus(null), 5000);
    },
    onError: () => {
      setLastRefreshStatus('Refresh failed');
      setTimeout(() => setLastRefreshStatus(null), 5000);
    },
  });

  // Fetch projects with periodic refresh
  const { data: projects = [], isLoading: projectsLoading } = useQuery({
    queryKey: ['projects'],
    queryFn: apiClient.getProjects,
    refetchInterval: 30000, // Refresh every 30 seconds
    refetchIntervalInBackground: true,
  });

  // Fetch targets for selected project with periodic refresh
  const { data: targets = [], isLoading: targetsLoading } = useQuery({
    queryKey: ['targets', selectedProjectId],
    queryFn: () => apiClient.getTargets(selectedProjectId!),
    enabled: !!selectedProjectId,
    refetchInterval: 30000, // Refresh every 30 seconds
    refetchIntervalInBackground: true,
  });

  const handleProjectChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const projectId = e.target.value ? Number(e.target.value) : null;
    onProjectChange(projectId);
    onTargetChange(null); // Reset target when project changes
  };

  const handleTargetChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const targetId = e.target.value ? Number(e.target.value) : null;
    onTargetChange(targetId);
  };

  return (
    <div className="project-target-selector">
      <div className="selector-group">
        <label htmlFor="project-select">Project:</label>
        <select
          id="project-select"
          value={selectedProjectId || ''}
          onChange={handleProjectChange}
          disabled={projectsLoading}
        >
          <option value="">Select a project</option>
          {projects.map(project => (
            <option 
              key={project.id} 
              value={project.id}
              disabled={!project.has_files}
            >
              {project.name} {!project.has_files && '(no files)'}
            </option>
          ))}
        </select>
      </div>

      <div className="selector-group">
        <label htmlFor="target-select">Target:</label>
        <select
          id="target-select"
          value={selectedTargetId || ''}
          onChange={handleTargetChange}
          disabled={!selectedProjectId || targetsLoading}
        >
          <option value="">All targets</option>
          {targets.map(target => (
            <option 
              key={target.id} 
              value={target.id}
              disabled={!target.has_files}
            >
              {target.name} ({target.accepted_count}/{target.image_count} accepted) {!target.has_files && '- no files'}
            </option>
          ))}
        </select>
      </div>

      {selectedProjectId && targets.length > 0 && (
        <div className="selection-stats">
          Total images: {targets.reduce((sum, t) => sum + t.image_count, 0)} | 
          Accepted: {targets.reduce((sum, t) => sum + t.accepted_count, 0)} | 
          Rejected: {targets.reduce((sum, t) => sum + t.rejected_count, 0)}
        </div>
      )}

      <div className="selector-actions">
        <button
          className="refresh-button"
          onClick={() => refreshCacheMutation.mutate()}
          disabled={refreshCacheMutation.isPending}
          title="Refresh file existence cache"
        >
          {refreshCacheMutation.isPending ? 'Refreshing...' : 'Refresh Files'}
        </button>
        {lastRefreshStatus && (
          <span className="refresh-status">{lastRefreshStatus}</span>
        )}
      </div>
    </div>
  );
}