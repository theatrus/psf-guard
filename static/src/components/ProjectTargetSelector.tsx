import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { Project, Target } from '../api/types';
import { useDbProjectTarget } from '../hooks/useUrlState';
import { useMergedProjects, useMergedTargets, type WithDb } from '../hooks/useDatabases';
import {
  groupProjectsByActivity,
  isArchivedProject,
  projectMatchesSearch,
  projectLastWorkedAt,
  sortProjects,
} from '../utils/projectNavigation';
import { formatRelativeTime } from '../utils/relativeTime';

type NavigationProject = WithDb<Project>;
type NavigationTarget = WithDb<Target>;

function projectKey(project: NavigationProject): string {
  return `${project.db_id}:${project.id}`;
}

interface ProjectTreeOptionProps {
  project: NavigationProject;
  targets: NavigationTarget[];
  targetsLoading: boolean;
  expanded: boolean;
  selectedDbId: string | null;
  selectedProjectId: number | null;
  selectedTargetId: number | null;
  relativeNow: number;
  onToggle: () => void;
  onChooseProject: () => void;
  onChooseTarget: (target: NavigationTarget) => void;
}

function ProjectTreeOption({
  project,
  targets,
  targetsLoading,
  expanded,
  selectedDbId,
  selectedProjectId,
  selectedTargetId,
  relativeNow,
  onToggle,
  onChooseProject,
  onChooseTarget,
}: ProjectTreeOptionProps) {
  const latest = projectLastWorkedAt(project);
  const projectSelected =
    selectedDbId === project.db_id &&
    selectedProjectId === project.id &&
    selectedTargetId === null;

  return (
    <div className="selector-project-tree">
      <div className="selector-project-row">
        <button
          type="button"
          className={`selector-option ${projectSelected ? 'is-selected' : ''}`}
          aria-current={projectSelected ? 'true' : undefined}
          disabled={!project.has_files}
          onClick={onChooseProject}
        >
          <span>{project.display_name}</span>
          <small>
            {project.db_name}
            {latest !== null ? ` · ${formatRelativeTime(latest, relativeNow)}` : ''}
            {!project.has_files ? ' · no files' : ''}
          </small>
        </button>
        <button
          type="button"
          className="selector-expand-button"
          aria-label={`${expanded ? 'Hide' : 'Show'} targets for ${project.display_name}`}
          aria-expanded={expanded}
          onClick={onToggle}
        >
          <span className={expanded ? 'expanded' : ''} aria-hidden="true">
            ▶
          </span>
        </button>
      </div>

      {expanded && (
        <div className="selector-project-targets">
          <button
            type="button"
            className={`selector-option selector-target-option ${projectSelected ? 'is-selected' : ''}`}
            aria-current={projectSelected ? 'true' : undefined}
            disabled={!project.has_files}
            onClick={onChooseProject}
          >
            <span>All images</span>
            <small>{project.display_name}</small>
          </button>

          {targets.map((target) => {
            const selected =
              selectedDbId === target.db_id &&
              selectedProjectId === target.project_id &&
              selectedTargetId === target.id;
            return (
              <button
                key={`${target.db_id}:${target.id}`}
                type="button"
                className={`selector-option selector-target-option ${selected ? 'is-selected' : ''}`}
                aria-current={selected ? 'true' : undefined}
                disabled={!target.has_files}
                onClick={() => onChooseTarget(target)}
              >
                <span>{target.name}</span>
                <small>
                  {target.accepted_count}/{target.image_count} accepted
                  {!target.has_files ? ' · no files' : ''}
                </small>
              </button>
            );
          })}

          {targetsLoading && <p className="selector-empty">Loading targets…</p>}
          {!targetsLoading && targets.length === 0 && (
            <p className="selector-empty">No matching targets.</p>
          )}
        </div>
      )}
    </div>
  );
}

export default function ProjectTargetSelector() {
  const {
    dbId,
    projectId: selectedProjectId,
    targetId: selectedTargetId,
    setDbProjectTarget,
  } = useDbProjectTarget();
  const queryClient = useQueryClient();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());
  const [search, setSearch] = useState('');
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
  const { data: targets, isLoading: targetsLoading } = useMergedTargets();

  useEffect(() => {
    const timer = window.setInterval(() => setRelativeNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const closePicker = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setPickerOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && pickerOpen) {
        setPickerOpen(false);
        triggerRef.current?.focus();
      }
    };
    document.addEventListener('pointerdown', closePicker);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closePicker);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [pickerOpen]);

  useEffect(() => {
    if (pickerOpen) searchRef.current?.focus();
  }, [pickerOpen]);

  const targetsByProject = useMemo(() => {
    const map = new Map<string, NavigationTarget[]>();
    for (const target of targets) {
      const key = `${target.db_id}:${target.project_id}`;
      const items = map.get(key) ?? [];
      items.push(target);
      map.set(key, items);
    }
    for (const items of map.values()) {
      items.sort((left, right) => left.name.localeCompare(right.name));
    }
    return map;
  }, [targets]);

  const normalizedSearch = search.trim().toLocaleLowerCase();
  const targetMatchesSearch = (target: NavigationTarget) =>
    !normalizedSearch || target.name.toLocaleLowerCase().includes(normalizedSearch);
  const projectTargets = (project: NavigationProject) =>
    targetsByProject.get(projectKey(project)) ?? [];
  const visibleTargets = (project: NavigationProject) => {
    const projectMatch = projectMatchesSearch(project, search);
    const allTargets = projectTargets(project);
    return projectMatch ? allTargets : allTargets.filter(targetMatchesSearch);
  };

  const matchingProjects = useMemo(
    () => {
      const query = search.trim().toLocaleLowerCase();
      return projects.filter((project) => {
        if (projectMatchesSearch(project, search)) return true;
        return (targetsByProject.get(projectKey(project)) ?? []).some((target) =>
          target.name.toLocaleLowerCase().includes(query)
        );
      });
    },
    [projects, search, targetsByProject]
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
  const matchingDatabases = useMemo(
    () =>
      (databases ?? []).filter(
        (database) =>
          !normalizedSearch ||
          database.name.toLocaleLowerCase().includes(normalizedSearch) ||
          database.id.toLocaleLowerCase().includes(normalizedSearch)
      ),
    [databases, normalizedSearch]
  );

  const selectedProject = projects.find(
    (project) => project.db_id === dbId && project.id === selectedProjectId
  );
  const selectedDatabase = databases?.find((database) => database.id === dbId);
  const selectedTarget = targets.find(
    (target) => target.db_id === dbId && target.id === selectedTargetId
  );

  const chooseProject = (nextDbId: string | null, nextProjectId: number | null) => {
    setDbProjectTarget(nextDbId, nextProjectId, null);
    setPickerOpen(false);
    setSearch('');
  };

  const chooseTarget = (target: NavigationTarget) => {
    setDbProjectTarget(target.db_id, target.project_id, target.id);
    setPickerOpen(false);
    setSearch('');
  };

  const toggleProject = (project: NavigationProject) => {
    const key = projectKey(project);
    setExpandedProjects((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const openPicker = () => {
    setPickerOpen((open) => {
      const next = !open;
      if (next && selectedProject) {
        setExpandedProjects((current) => new Set(current).add(projectKey(selectedProject)));
      }
      return next;
    });
  };

  const scopeLabel = projectsLoading
    ? 'Loading projects…'
    : selectedTarget && selectedProject
      ? `${selectedProject.display_name} · ${selectedTarget.name}`
      : selectedProject?.display_name ??
        (selectedDatabase ? `All projects · ${selectedDatabase.name}` : 'Choose project or target');
  const refreshPending = refreshCacheMutation.isPending || refreshBothCachesMutation.isPending;

  const renderProject = (project: NavigationProject) => {
    const targetsForProject = visibleTargets(project);
    const searchFindsTarget =
      Boolean(normalizedSearch) && projectTargets(project).some(targetMatchesSearch);
    const expanded = expandedProjects.has(projectKey(project)) || searchFindsTarget;
    return (
      <ProjectTreeOption
        key={projectKey(project)}
        project={project}
        targets={targetsForProject}
        targetsLoading={targetsLoading}
        expanded={expanded}
        selectedDbId={dbId}
        selectedProjectId={selectedProjectId}
        selectedTargetId={selectedTargetId}
        relativeNow={relativeNow}
        onToggle={() => toggleProject(project)}
        onChooseProject={() => chooseProject(project.db_id, project.id)}
        onChooseTarget={chooseTarget}
      />
    );
  };

  return (
    <div ref={rootRef} className="project-target-selector compact combined-selector">
      <div className="selector-group compact selector-picker">
        <label id="scope-select-label" htmlFor="scope-select">
          Project / target:
        </label>
        <button
          ref={triggerRef}
          id="scope-select"
          type="button"
          className="compact-select selector-trigger"
          aria-labelledby="scope-select-label scope-select"
          aria-haspopup="dialog"
          aria-expanded={pickerOpen}
          disabled={projectsLoading}
          onClick={openPicker}
        >
          <span>{scopeLabel}</span>
          <span aria-hidden="true">▾</span>
        </button>

        {pickerOpen && (
          <div
            className="selector-popover combined-selector-popover"
            role="dialog"
            aria-label="Choose a project or target"
          >
            <input
              ref={searchRef}
              type="search"
              className="selector-search"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Type to find a project or target"
              aria-label="Search projects or targets"
            />
            <div className="selector-options" aria-label="Projects and targets">
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
                  {group.projects.map(renderProject)}
                </section>
              ))}

              {archivedProjects.length > 0 && (
                <details
                  className="selector-archive"
                  open={archiveOpen || Boolean(search)}
                  onToggle={(event) => setArchiveOpen(event.currentTarget.open)}
                >
                  <summary>
                    Archived projects <span>{archivedProjects.length}</span>
                  </summary>
                  {archivedProjects.map(renderProject)}
                </details>
              )}

              {projectGroups.length === 0 &&
                archivedProjects.length === 0 &&
                matchingDatabases.length === 0 && (
                  <p className="selector-empty">No projects or targets match.</p>
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
