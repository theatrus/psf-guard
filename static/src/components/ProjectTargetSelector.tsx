import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useDbProjectTarget } from '../hooks/useUrlState';
import { useMergedProjects } from '../hooks/useDatabases';
import {
  groupProjectsByActivity,
  isArchivedProject,
  projectMatchesSearch,
  projectLastWorkedAt,
  sortProjects,
} from '../utils/projectNavigation';
import { formatRelativeTime } from '../utils/relativeTime';

export default function ProjectTargetSelector() {
  const {
    dbId,
    projectId: selectedProjectId,
    targetId: selectedTargetId,
    setDbProjectTarget,
    setTargetId,
  } = useDbProjectTarget();
  const queryClient = useQueryClient();
  const rootRef = useRef<HTMLDivElement>(null);
  const projectTriggerRef = useRef<HTMLButtonElement>(null);
  const targetTriggerRef = useRef<HTMLButtonElement>(null);
  const projectSearchRef = useRef<HTMLInputElement>(null);
  const targetSearchRef = useRef<HTMLInputElement>(null);
  const [projectOpen, setProjectOpen] = useState(false);
  const [targetOpen, setTargetOpen] = useState(false);
  const [projectArchiveOpen, setProjectArchiveOpen] = useState(false);
  const [projectSearch, setProjectSearch] = useState('');
  const [targetSearch, setTargetSearch] = useState('');
  const [relativeNow, setRelativeNow] = useState(Date.now);

  const invalidateAllForDb = () => {
    if (!dbId) return;
    queryClient.invalidateQueries({ queryKey: ['db', dbId] });
  };

  const refreshCacheMutation = useMutation({
    mutationFn: () => apiClient.refreshFileCache(dbId!),
    onSuccess: invalidateAllForDb,
    onError: (error) => console.error('File cache refresh failed:', error),
  });

  const refreshBothCachesMutation = useMutation({
    mutationFn: async () => {
      await apiClient.refreshDirectoryCache(dbId!);
      await apiClient.refreshFileCache(dbId!);
    },
    onSuccess: invalidateAllForDb,
    onError: (error) => console.error('Combined cache refresh failed:', error),
  });

  const { data: projects, databases, isLoading: projectsLoading } = useMergedProjects();
  const { data: targets = [], isLoading: targetsLoading } = useQuery({
    queryKey: ['db', dbId, 'targets', selectedProjectId],
    queryFn: () => apiClient.getTargets(dbId!, selectedProjectId!),
    enabled: !!dbId && !!selectedProjectId,
    refetchInterval: 30000,
    refetchIntervalInBackground: true,
  });

  useEffect(() => {
    const timer = window.setInterval(() => setRelativeNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const closeMenus = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setProjectOpen(false);
        setTargetOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setProjectOpen(false);
        setTargetOpen(false);
        if (projectOpen) projectTriggerRef.current?.focus();
        else if (targetOpen) targetTriggerRef.current?.focus();
      }
    };
    document.addEventListener('pointerdown', closeMenus);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeMenus);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [projectOpen, targetOpen]);

  useEffect(() => {
    if (projectOpen) projectSearchRef.current?.focus();
  }, [projectOpen]);

  useEffect(() => {
    if (targetOpen) targetSearchRef.current?.focus();
  }, [targetOpen]);

  const selectedProject = projects.find(
    (project) => project.db_id === dbId && project.id === selectedProjectId
  );
  const selectedDatabase = databases?.find((database) => database.id === dbId);
  const selectedTarget = targets.find((target) => target.id === selectedTargetId);

  const matchingProjects = useMemo(
    () => projects.filter((project) => projectMatchesSearch(project, projectSearch)),
    [projectSearch, projects]
  );
  const projectGroups = useMemo(
    () =>
      groupProjectsByActivity(
        matchingProjects.filter((project) => !isArchivedProject(project)),
        relativeNow
      ),
    [matchingProjects, relativeNow]
  );
  const archivedProjects = useMemo(
    () => sortProjects(matchingProjects.filter(isArchivedProject), 'recent'),
    [matchingProjects]
  );
  const matchingDatabases = useMemo(() => {
    const search = projectSearch.trim().toLocaleLowerCase();
    return (databases ?? []).filter(
      (database) =>
        !search ||
        database.name.toLocaleLowerCase().includes(search) ||
        database.id.toLocaleLowerCase().includes(search)
    );
  }, [databases, projectSearch]);
  const matchingTargets = useMemo(() => {
    const search = targetSearch.trim().toLocaleLowerCase();
    return targets.filter((target) => !search || target.name.toLocaleLowerCase().includes(search));
  }, [targetSearch, targets]);

  const chooseProject = (nextDbId: string | null, nextProjectId: number | null) => {
    setDbProjectTarget(nextDbId, nextProjectId, null);
    setProjectOpen(false);
    setProjectSearch('');
  };

  const chooseTarget = (targetId: number | null) => {
    setTargetId(targetId);
    setTargetOpen(false);
    setTargetSearch('');
  };

  const projectLabel = projectsLoading
    ? 'Loading projects…'
    : selectedProject?.display_name ??
      (selectedDatabase ? `All projects · ${selectedDatabase.name}` : 'Choose project');
  const targetLabel =
    selectedProjectId === null
      ? 'All targets'
      : targetsLoading
        ? 'Loading targets…'
        : selectedTarget?.name ?? 'All targets';
  const refreshPending = refreshCacheMutation.isPending || refreshBothCachesMutation.isPending;

  return (
    <div ref={rootRef} className="project-target-selector compact">
      <div className="selector-group compact selector-picker">
        <label id="project-select-label" htmlFor="project-select">
          Project:
        </label>
        <button
          ref={projectTriggerRef}
          id="project-select"
          type="button"
          className="compact-select selector-trigger"
          aria-labelledby="project-select-label project-select"
          aria-haspopup="dialog"
          aria-expanded={projectOpen}
          disabled={projectsLoading}
          onClick={() => {
            setProjectOpen((open) => !open);
            setTargetOpen(false);
          }}
        >
          <span>{projectLabel}</span>
          <span aria-hidden="true">▾</span>
        </button>

        {projectOpen && (
          <div
            className="selector-popover project-selector-popover"
            role="dialog"
            aria-label="Choose a project"
          >
            <input
              ref={projectSearchRef}
              type="search"
              className="selector-search"
              value={projectSearch}
              onChange={(event) => setProjectSearch(event.target.value)}
              placeholder="Type to find a project"
              aria-label="Search projects"
            />
            <div className="selector-options" aria-label="Projects">
              <button
                type="button"
                className={`selector-option ${dbId === null ? 'is-selected' : ''}`}
                aria-current={dbId === null ? 'true' : undefined}
                onClick={() => chooseProject(null, null)}
              >
                <span>Choose a project</span>
                <small>Show all databases</small>
              </button>
              {matchingDatabases.map((database) => (
                <button
                  key={`${database.id}:all`}
                  type="button"
                  className={`selector-option ${dbId === database.id && selectedProjectId === null ? 'is-selected' : ''}`}
                  aria-current={
                    dbId === database.id && selectedProjectId === null ? 'true' : undefined
                  }
                  onClick={() => chooseProject(database.id, null)}
                >
                  <span>All projects</span>
                  <small>{database.name}</small>
                </button>
              ))}

              {projectGroups.map((group) => (
                <section
                  key={group.id}
                  className={`selector-option-group ${group.id === 'recent' ? 'is-recent' : ''}`}
                  aria-label={group.label}
                >
                  <div className="selector-group-heading">
                    <span>{group.label}</span>
                    <span>{group.projects.length}</span>
                  </div>
                  {group.projects.map((project) => {
                    const latest = projectLastWorkedAt(project);
                    return (
                      <button
                        key={`${project.db_id}:${project.id}`}
                        type="button"
                        className={`selector-option ${dbId === project.db_id && selectedProjectId === project.id ? 'is-selected' : ''}`}
                        aria-current={
                          dbId === project.db_id && selectedProjectId === project.id
                            ? 'true'
                            : undefined
                        }
                        disabled={!project.has_files}
                        onClick={() => chooseProject(project.db_id, project.id)}
                      >
                        <span>{project.display_name}</span>
                        <small>
                          {project.db_name}
                          {latest !== null ? ` · ${formatRelativeTime(latest, relativeNow)}` : ''}
                          {!project.has_files ? ' · no files' : ''}
                        </small>
                      </button>
                    );
                  })}
                </section>
              ))}

              {archivedProjects.length > 0 && (
                <details
                  className="selector-archive"
                  open={projectArchiveOpen || Boolean(projectSearch)}
                  onToggle={(event) => setProjectArchiveOpen(event.currentTarget.open)}
                >
                  <summary>Archived projects <span>{archivedProjects.length}</span></summary>
                  {archivedProjects.map((project) => (
                    <button
                      key={`${project.db_id}:${project.id}`}
                      type="button"
                      className="selector-option"
                      aria-current={
                        dbId === project.db_id && selectedProjectId === project.id
                          ? 'true'
                          : undefined
                      }
                      disabled={!project.has_files}
                      onClick={() => chooseProject(project.db_id, project.id)}
                    >
                      <span>{project.display_name}</span>
                      <small>{project.db_name}</small>
                    </button>
                  ))}
                </details>
              )}

              {projectGroups.length === 0 &&
                archivedProjects.length === 0 &&
                matchingDatabases.length === 0 && (
                  <p className="selector-empty">No projects match.</p>
                )}
            </div>
          </div>
        )}
      </div>

      <div className="selector-group compact selector-picker">
        <label id="target-select-label" htmlFor="target-select">
          Target:
        </label>
        <button
          ref={targetTriggerRef}
          id="target-select"
          type="button"
          className="compact-select selector-trigger"
          aria-labelledby="target-select-label target-select"
          aria-haspopup="dialog"
          aria-expanded={targetOpen}
          disabled={!selectedProjectId || targetsLoading}
          onClick={() => {
            setTargetOpen((open) => !open);
            setProjectOpen(false);
          }}
        >
          <span>{targetLabel}</span>
          <span aria-hidden="true">▾</span>
        </button>

        {targetOpen && (
          <div
            className="selector-popover target-selector-popover"
            role="dialog"
            aria-label="Choose a target"
          >
            <input
              ref={targetSearchRef}
              type="search"
              className="selector-search"
              value={targetSearch}
              onChange={(event) => setTargetSearch(event.target.value)}
              placeholder="Type to find a target"
              aria-label="Search targets"
            />
            <div className="selector-options" aria-label="Targets">
              <button
                type="button"
                className={`selector-option ${selectedTargetId === null ? 'is-selected' : ''}`}
                aria-current={selectedTargetId === null ? 'true' : undefined}
                onClick={() => chooseTarget(null)}
              >
                <span>All targets</span>
                <small>{selectedProject?.display_name}</small>
              </button>
              {matchingTargets.map((target) => (
                <button
                  key={target.id}
                  type="button"
                  className={`selector-option ${selectedTargetId === target.id ? 'is-selected' : ''}`}
                  aria-current={selectedTargetId === target.id ? 'true' : undefined}
                  disabled={!target.has_files}
                  onClick={() => chooseTarget(target.id)}
                >
                  <span>{target.name}</span>
                  <small>
                    {target.accepted_count}/{target.image_count} accepted
                    {!target.has_files ? ' · no files' : ''}
                  </small>
                </button>
              ))}
              {matchingTargets.length === 0 && (
                <p className="selector-empty">No targets match.</p>
              )}
            </div>
          </div>
        )}
      </div>

      <button
        className="refresh-button compact"
        onClick={(event) => {
          if (event.shiftKey) refreshBothCachesMutation.mutate();
          else refreshCacheMutation.mutate();
        }}
        disabled={!dbId || refreshPending}
        title={
          refreshBothCachesMutation.isPending
            ? 'Refreshing directory and file caches...'
            : refreshCacheMutation.isPending
              ? 'Refreshing file cache...'
              : 'Refresh file cache (Shift+Click for directory + file cache)'
        }
      >
        {refreshPending ? '⟳' : '↻'}
      </button>
    </div>
  );
}
