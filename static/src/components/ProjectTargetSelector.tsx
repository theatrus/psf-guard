import { useMemo } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useDbProjectTarget } from '../hooks/useUrlState';
import { useMergedProjects, type WithDb } from '../hooks/useDatabases';
import type { Project } from '../api/types';

export default function ProjectTargetSelector() {
  const {
    dbId,
    projectId: selectedProjectId,
    targetId: selectedTargetId,
    setDbProjectTarget,
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

  // Fetch projects from EVERY configured database so the dropdown spans DBs.
  const { data: projects, databases, isLoading: projectsLoading } = useMergedProjects();

  // Group projects by their source DB so each renders under its own optgroup.
  const projectsByDb = useMemo(() => {
    const map: Record<string, WithDb<Project>[]> = {};
    for (const p of projects) {
      (map[p.db_id] ||= []).push(p);
    }
    return map;
  }, [projects]);

  // Fetch targets for selected project with periodic refresh
  const { data: targets = [], isLoading: targetsLoading } = useQuery({
    queryKey: ['db', dbId, 'targets', selectedProjectId],
    queryFn: () => apiClient.getTargets(dbId!, selectedProjectId!),
    enabled: !!dbId && !!selectedProjectId,
    refetchInterval: 30000, // Refresh every 30 seconds
    refetchIntervalInBackground: true,
  });

  // Option values encode the source DB since project IDs collide across DBs:
  //   ""              -> placeholder (clears the scope)
  //   "<db>:all"      -> all projects in that DB
  //   "<db>:<number>" -> a specific project in that DB
  const handleProjectChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const value = e.target.value;
    if (!value) {
      setDbProjectTarget(null, null, null);
      return;
    }
    const sep = value.indexOf(':');
    const db = value.slice(0, sep);
    const rest = value.slice(sep + 1);
    const projectId = rest === 'all' ? null : Number(rest);
    // Switching project (or DB) always resets the target.
    setDbProjectTarget(db, projectId, null);
  };

  // Reflect the current (db, project) selection back onto the <select> value.
  const projectValue =
    dbId == null
      ? ''
      : selectedProjectId === null
        ? `${dbId}:all`
        : selectedProjectId
          ? `${dbId}:${selectedProjectId}`
          : '';

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
          value={projectValue}
          onChange={handleProjectChange}
          disabled={projectsLoading}
        >
          <option value="">Select project</option>
          {(databases ?? []).map(db => (
            <optgroup key={db.id} label={db.name}>
              <option value={`${db.id}:all`}>- All Projects -</option>
              {(projectsByDb[db.id] ?? []).map(project => (
                <option
                  key={`${db.id}:${project.id}`}
                  value={`${db.id}:${project.id}`}
                  disabled={!project.has_files}
                >
                  {project.display_name} {!project.has_files && '(no files)'}
                </option>
              ))}
            </optgroup>
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
        disabled={!dbId || refreshCacheMutation.isPending || refreshDirectoryCacheMutation.isPending || refreshBothCachesMutation.isPending}
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