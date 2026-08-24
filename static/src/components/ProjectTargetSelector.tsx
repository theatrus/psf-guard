import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useDbProjectTarget } from '../hooks/useUrlState';
import { useMergedProjects, useMergedTargets } from '../hooks/useDatabases';
import {
  buildProjectTargetNavigation,
  projectNavigationKey,
  type NavigationProject,
  type NavigationTarget,
} from '../utils/projectTargetNavigation';
import { useAccess } from '../auth/access';
import { useDisplayPreferences } from '../hooks/useDisplayPreferences';
import ProjectTreeOption from './projectSelector/ProjectTreeOption';

export default function ProjectTargetSelector() {
  const {
    dbId,
    projectId: selectedProjectId,
    targetId: selectedTargetId,
    setDbProjectTarget,
  } = useDbProjectTarget();
  const access = useAccess();
  const queryClient = useQueryClient();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const optionsRef = useRef<HTMLDivElement>(null);
  const revealedSelectionRef = useRef(false);
  const searchExpansionSeedRef = useRef<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());
  const [search, setSearch] = useState('');
  const [relativeNow, setRelativeNow] = useState(Date.now);
  // How the list is organized — by activity or by database — set in
  // Settings → Review and stored in this browser.
  const { projectPickerGrouping } = useDisplayPreferences();

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
  const {
    data: targets,
    isLoading: targetsLoading,
    isError: targetsError,
  } = useMergedTargets();

  const navigation = useMemo(
    () =>
      buildProjectTargetNavigation({
        projects,
        targets,
        databases: databases ?? [],
        search,
        relativeNow,
        grouping: projectPickerGrouping,
      }),
    [projects, targets, databases, search, relativeNow, projectPickerGrouping]
  );

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

  // Opening the picker lands on the current selection, not the top of the
  // list. The selected row is the one carrying aria-current; a selected
  // target's row mounts only after its targets load, so this waits on the
  // navigation model and scrolls once per open.
  useEffect(() => {
    // No database in scope means the current row is "Choose a project" at the
    // very top, which needs no scrolling to be seen.
    if (!pickerOpen || dbId === null) {
      revealedSelectionRef.current = false;
      return;
    }
    if (revealedSelectionRef.current) return;
    const current = optionsRef.current?.querySelector<HTMLElement>('[aria-current="true"]');
    if (!current) return;
    revealedSelectionRef.current = true;
    // jsdom has no layout, so guard for tests and older embedded webviews.
    current.scrollIntoView?.({ block: 'center' });
  }, [pickerOpen, dbId, navigation, targetsLoading]);

  // Seed expansion once for each search, and once more if target rows finish
  // loading after the user starts typing. Explicit state owns expansion after
  // that, so the user can collapse a search result without it reopening.
  useEffect(() => {
    if (!navigation.normalizedSearch) {
      searchExpansionSeedRef.current = null;
      return;
    }
    const seed = `${navigation.normalizedSearch}:${targetsLoading ? 'loading' : 'ready'}`;
    if (searchExpansionSeedRef.current === seed) return;
    searchExpansionSeedRef.current = seed;
    setExpandedProjects((current) => {
      const next = new Set(current);
      for (const key of navigation.matchingTargetProjectKeys) next.add(key);
      return next;
    });
  }, [navigation, targetsLoading]);

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
    const key = projectNavigationKey(project);
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
        setExpandedProjects((current) =>
          new Set(current).add(projectNavigationKey(selectedProject))
        );
      }
      return next;
    });
  };

  const retryTargets = () => {
    queryClient.invalidateQueries({
      predicate: (query) =>
        query.queryKey[0] === 'db' && query.queryKey[2] === 'target-navigation',
    });
  };

  const scopeLabel = projectsLoading
    ? 'Loading projects…'
    : selectedTarget && selectedProject
      ? `${selectedProject.display_name} · ${selectedTarget.name}`
      : selectedProject?.display_name ??
        (selectedDatabase ? 'All projects' : 'Choose project or target');
  // Closed, the trigger is the only place the current database is named, and
  // project names repeat across catalogs. Every row inside the picker already
  // carries its database this way.
  const scopeDatabase = projectsLoading
    ? null
    : selectedProject?.db_name ?? selectedDatabase?.name ?? null;
  const refreshPending = refreshCacheMutation.isPending || refreshBothCachesMutation.isPending;

  const renderProject = (project: NavigationProject) => (
    <ProjectTreeOption
      key={projectNavigationKey(project)}
      project={project}
      targets={navigation.targetsForProject(project)}
      targetsLoading={targetsLoading}
      targetsError={targetsError}
      expanded={expandedProjects.has(projectNavigationKey(project))}
      selectedDbId={dbId}
      selectedProjectId={selectedProjectId}
      selectedTargetId={selectedTargetId}
      relativeNow={relativeNow}
      onToggle={() => toggleProject(project)}
      onChooseProject={() => chooseProject(project.db_id, project.id)}
      onChooseTarget={chooseTarget}
    />
  );

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
          <span className="selector-trigger-scope">
            <span>{scopeLabel}</span>
            {scopeDatabase && <small>{scopeDatabase}</small>}
          </span>
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
            {targetsError && (
              <div className="selector-load-error" role="alert">
                <span>Some targets could not be loaded.</span>
                <button type="button" onClick={retryTargets}>
                  Retry
                </button>
              </div>
            )}
            <div ref={optionsRef} className="selector-options" aria-label="Projects and targets">
              <button
                type="button"
                className={`selector-option ${dbId === null ? 'is-selected' : ''}`}
                aria-current={dbId === null ? 'true' : undefined}
                onClick={() => chooseProject(null, null)}
              >
                <span>Choose a project</span>
                <small>Show all databases</small>
              </button>
              {navigation.matchingDatabases.map((database) => (
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

              {navigation.projectGroups.map((group) => (
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

              {navigation.archivedProjects.length > 0 && (
                <details
                  className="selector-archive"
                  open={archiveOpen || Boolean(search)}
                  onToggle={(event) => setArchiveOpen(event.currentTarget.open)}
                >
                  <summary>
                    Archived projects <span>{navigation.archivedProjects.length}</span>
                  </summary>
                  {navigation.archivedProjects.map(renderProject)}
                </details>
              )}

              {navigation.projectGroups.length === 0 &&
                navigation.archivedProjects.length === 0 &&
                navigation.matchingDatabases.length === 0 && (
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
        disabled={!access.canWrite || !dbId || refreshPending}
        title={
          !access.canWrite
            ? 'A read-only account cannot refresh file caches.'
            : refreshBothCachesMutation.isPending
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
