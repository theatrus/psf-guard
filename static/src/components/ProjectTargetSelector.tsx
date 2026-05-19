import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useDbProjectTarget } from '../hooks/useUrlState';

export default function ProjectTargetSelector() {
  const {
    dbId,
    projectId: selectedProjectId,
    targetId: selectedTargetId,
    setProjectId,
    setTargetId,
  } = useDbProjectTarget();
  const queryClient = useQueryClient();

  const invalidateAllForDb = () => {
    if (!dbId) return;
    queryClient.invalidateQueries({ queryKey: ['db', dbId] });
  };

  // Refresh file cache mutation
  const refreshCacheMutation = useMutation({
    mutationFn: () => apiClient.refreshFileCache(dbId!),
    onSuccess: () => {
      console.log('🔄 Manual file cache refresh completed, invalidating queries...');
      invalidateAllForDb();
    },
    onError: (error) => {
      console.error('File cache refresh failed:', error);
    },
  });

  // Refresh directory cache mutation
  const refreshDirectoryCacheMutation = useMutation({
    mutationFn: () => apiClient.refreshDirectoryCache(dbId!),
    onSuccess: () => {
      console.log('🌳 Directory cache refresh completed, invalidating queries...');
      invalidateAllForDb();
    },
    onError: (error) => {
      console.error('Directory cache refresh failed:', error);
    },
  });

  // Combined refresh for shift-click
  const refreshBothCachesMutation = useMutation({
    mutationFn: async () => {
      // Refresh directory cache first, then file cache
      await apiClient.refreshDirectoryCache(dbId!);
      await apiClient.refreshFileCache(dbId!);
    },
    onSuccess: () => {
      console.log('🔄 Combined cache refresh completed, invalidating queries...');
      invalidateAllForDb();
    },
    onError: (error) => {
      console.error('Combined cache refresh failed:', error);
    },
  });

  // Fetch projects with periodic refresh
  const { data: projects = [], isLoading: projectsLoading } = useQuery({
    queryKey: ['db', dbId, 'projects'],
    queryFn: () => apiClient.getProjects(dbId!),
    enabled: !!dbId,
    refetchInterval: 30000, // Refresh every 30 seconds
    refetchIntervalInBackground: true,
  });

  // Fetch targets for selected project with periodic refresh
  const { data: targets = [], isLoading: targetsLoading } = useQuery({
    queryKey: ['db', dbId, 'targets', selectedProjectId],
    queryFn: () => apiClient.getTargets(dbId!, selectedProjectId!),
    enabled: !!dbId && !!selectedProjectId,
    refetchInterval: 30000, // Refresh every 30 seconds
    refetchIntervalInBackground: true,
  });

  const handleProjectChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const value = e.target.value;
    let projectId: number | null = null;
    
    if (value === 'all') {
      projectId = null; // null means all projects
    } else if (value) {
      projectId = Number(value);
    }
    
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
          value={selectedProjectId === null ? 'all' : selectedProjectId || ''}
          onChange={handleProjectChange}
          disabled={projectsLoading}
        >
          <option value="">Select project</option>
          <option value="all">- All Projects -</option>
          {projects.map(project => (
            <option 
              key={project.id} 
              value={project.id}
              disabled={!project.has_files}
            >
              {project.display_name} {!project.has_files && '(no files)'}
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
          disabled={selectedProjectId === null || !selectedProjectId || targetsLoading}
        >
          <option value="">{selectedProjectId === null ? 'All projects selected' : 'All targets'}</option>
          {selectedProjectId !== null && targets.map(target => (
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
        onClick={(e) => {
          if (e.shiftKey) {
            refreshBothCachesMutation.mutate();
          } else {
            refreshCacheMutation.mutate();
          }
        }}
        disabled={refreshCacheMutation.isPending || refreshDirectoryCacheMutation.isPending || refreshBothCachesMutation.isPending}
        title={
          refreshBothCachesMutation.isPending 
            ? 'Refreshing directory and file caches...'
            : refreshDirectoryCacheMutation.isPending
            ? 'Refreshing directory cache...'
            : refreshCacheMutation.isPending
            ? 'Refreshing file cache...'
            : 'Refresh file cache (Shift+Click for directory + file cache)'
        }
      >
        {(refreshCacheMutation.isPending || refreshDirectoryCacheMutation.isPending || refreshBothCachesMutation.isPending) ? '⟳' : '↻'}
      </button>
    </div>
  );
}