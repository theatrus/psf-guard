import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useProjectTarget } from '../hooks/useUrlState';

export default function ProjectTargetSelector() {
  const { projectId: selectedProjectId, targetId: selectedTargetId, setProjectId, setTargetId } = useProjectTarget();
  const queryClient = useQueryClient();

  // Refresh cache mutation
  const refreshCacheMutation = useMutation({
    mutationFn: apiClient.refreshFileCache,
    onSuccess: () => {
      // Invalidate and refetch projects and targets
      queryClient.invalidateQueries({ queryKey: ['projects'] });
      queryClient.invalidateQueries({ queryKey: ['targets'] });
    },
    onError: (error) => {
      console.error('Cache refresh failed:', error);
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
    setProjectId(projectId);
  };

  const handleTargetChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const targetId = e.target.value ? Number(e.target.value) : null;
    setTargetId(targetId);
  };

  return (
    <div className="project-target-selector compact">
      <div className="selector-group compact">
        <label htmlFor="project-select">Project:</label>
        <select
          id="project-select"
          className="compact-select"
          value={selectedProjectId || ''}
          onChange={handleProjectChange}
          disabled={projectsLoading}
        >
          <option value="">Select project</option>
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

      <div className="selector-group compact">
        <label htmlFor="target-select">Target:</label>
        <select
          id="target-select"
          className="compact-select"
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
              {target.name} ({target.accepted_count}/{target.image_count})
            </option>
          ))}
        </select>
      </div>

      <button
        className="refresh-button compact"
        onClick={() => refreshCacheMutation.mutate()}
        disabled={refreshCacheMutation.isPending}
        title={refreshCacheMutation.isPending ? 'Refreshing file cache...' : 'Refresh file cache'}
      >
        {refreshCacheMutation.isPending ? '⟳' : '↻'}
      </button>
    </div>
  );
}