import { useEffect, useState, useMemo } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from 'react-router-dom';
import { apiClient } from '../api/client';
import type { ProjectOverview, TargetOverview, DateRange } from '../api/types';
import {
  useAllDatabases,
  useMergedProjectsOverview,
  useMergedTargetsOverview,
  useMergedOverallStats,
  type WithDb,
} from '../hooks/useDatabases';
import { isTauriApp, tauriFileSystem } from '../utils/tauri';
import {
  loadProjectSeenState,
  markerForProject,
  newImageCount,
  projectSeenKey,
  saveProjectSeenState,
} from '../utils/projectRecency';
import ProjectSchedulerDialog from './ProjectSchedulerDialog';
import './Overview.css';

/// Inline edit state for correcting imported groupings.
type Organizing =
  | { kind: 'project'; dbId: string; id: number; name: string; mergeInto: string }
  | { kind: 'target'; dbId: string; id: number; name: string; moveTo: string };

export default function Overview() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());
  const [collapsedDbs, setCollapsedDbs] = useState<Set<string>>(new Set());
  const [organizing, setOrganizing] = useState<Organizing | null>(null);
  const [organizeBusy, setOrganizeBusy] = useState(false);
  const [organizeError, setOrganizeError] = useState('');
  const [seenProjects, setSeenProjects] = useState(loadProjectSeenState);
  const [schedulerProject, setSchedulerProject] = useState<{
    dbId: string;
    id: number;
    name: string;
  } | null>(null);

  const { data: databases } = useAllDatabases();
  const { data: serverInfo } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });
  const { data: overallStats, isLoading: statsLoading } = useMergedOverallStats();
  const { data: projects, isLoading: projectsLoading } = useMergedProjectsOverview();
  const { data: targets, isLoading: targetsLoading } = useMergedTargetsOverview();
  const organizeAllowed = serverInfo?.allow_database_management ?? false;

  // The first time this browser sees a project, record its current image
  // count without calling the whole back catalog new. Later refreshes compare
  // against that marker until the user opens the project.
  useEffect(() => {
    setSeenProjects((current) => {
      const next = { ...current };
      let changed = false;

      for (const project of projects) {
        const key = projectSeenKey(project.db_id, project.id);
        if (!next[key]) {
          next[key] = markerForProject(project);
          changed = true;
        }
      }

      if (changed) saveProjectSeenState(next);
      return changed ? next : current;
    });
  }, [projects]);

  // Desktop mode: export straight to a local folder (hardlink-or-copy) via
  // the native picker — the server IS this machine, so downloading a zip of
  // our own files would be silly. Browser mode keeps the zip download link.
  const isTauri = isTauriApp();
  const [exportBusy, setExportBusy] = useState(false);
  const handleLocalExport = async (
    dbId: string,
    scope: { project_id?: number; target_id?: number },
    label: string
  ) => {
    try {
      const dest = await tauriFileSystem.pickImageDirectory();
      if (!dest) return;
      setExportBusy(true);
      const summary = await apiClient.exportLocal(dbId, { dest, ...scope });
      const placed = summary.copied + summary.linked;
      alert(
        `Exported ${label}: ${placed} file(s) placed` +
          `${summary.linked > 0 ? ` (${summary.linked} hardlinked)` : ''}` +
          `${summary.skipped_existing > 0 ? `, ${summary.skipped_existing} already present` : ''}` +
          `${summary.missing > 0 ? `, ${summary.missing} missing on disk` : ''}` +
          `${summary.errors > 0 ? `, ${summary.errors} ERRORS` : ''}\n\n${dest}`
      );
    } catch (err) {
      alert(`Export failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setExportBusy(false);
    }
  };

  // Persist an organize edit (rename / move / merge), then refresh this DB's
  // overview queries so the new grouping shows up.
  const saveOrganize = async () => {
    if (!organizing) return;
    setOrganizeBusy(true);
    setOrganizeError('');
    try {
      if (organizing.kind === 'project') {
        if (organizing.mergeInto !== '') {
          if (
            !confirm(
              'Merge this project into the selected one? Its targets and images move over and this project is deleted.'
            )
          ) {
            setOrganizeBusy(false);
            return;
          }
          await apiClient.mergeProject(
            organizing.dbId,
            organizing.id,
            Number(organizing.mergeInto)
          );
        } else if (organizing.name.trim()) {
          await apiClient.updateProject(organizing.dbId, organizing.id, organizing.name.trim());
        }
      } else {
        const req: { name?: string; project_id?: number } = {};
        if (organizing.name.trim()) req.name = organizing.name.trim();
        if (organizing.moveTo !== '') req.project_id = Number(organizing.moveTo);
        if (req.name !== undefined || req.project_id !== undefined) {
          await apiClient.updateTarget(organizing.dbId, organizing.id, req);
        }
      }
      queryClient.invalidateQueries({ queryKey: ['db', organizing.dbId] });
      setOrganizing(null);
    } catch (err) {
      setOrganizeError(err instanceof Error ? err.message : String(err));
    } finally {
      setOrganizeBusy(false);
    }
  };

  // Group targets by (db_id, project_id) since project IDs collide across DBs.
  const targetsByProject = useMemo(() => {
    const map: Record<string, WithDb<TargetOverview>[]> = {};
    for (const target of targets) {
      const key = `${target.db_id}:${target.project_id}`;
      (map[key] ||= []).push(target);
    }
    return map;
  }, [targets]);

  // Group projects by their source DB so each section renders together.
  const projectsByDb = useMemo(() => {
    const map: Record<string, WithDb<ProjectOverview>[]> = {};
    for (const p of projects) {
      (map[p.db_id] ||= []).push(p);
    }
    return map;
  }, [projects]);

  // Helper functions
  const formatDate = (timestamp?: number) => {
    if (!timestamp) return 'Unknown';
    return new Date(timestamp * 1000).toLocaleDateString();
  };

  const formatDateRange = (dateRange: DateRange) => {
    if (!dateRange.earliest || !dateRange.latest) return 'No dates';
    const start = formatDate(dateRange.earliest);
    const end = formatDate(dateRange.latest);
    const span = dateRange.span_days ? `${dateRange.span_days} days` : '';
    return span ? `${start} - ${end} (${span})` : `${start} - ${end}`;
  };

  const getGradingProgress = (accepted: number, rejected: number, pending: number) => {
    const total = accepted + rejected + pending;
    if (total === 0) return { acceptedPct: 0, rejectedPct: 0, pendingPct: 0 };
    
    return {
      acceptedPct: Math.round((accepted / total) * 100),
      rejectedPct: Math.round((rejected / total) * 100),
      pendingPct: Math.round((pending / total) * 100),
    };
  };

  const getDesiredProgress = (accepted: number, desired: number) => {
    if (desired === 0) return 0;
    return Math.round((accepted / desired) * 100);
  };

  // Navigation handlers. Each click carries the project's db_id so the scoped
  // view knows which database to query.
  const markProjectSeen = (project: WithDb<ProjectOverview>) => {
    setSeenProjects((current) => {
      const next = {
        ...current,
        [projectSeenKey(project.db_id, project.id)]: markerForProject(project),
      };
      saveProjectSeenState(next);
      return next;
    });
  };

  const handleSelectProject = (project: WithDb<ProjectOverview>) => {
    markProjectSeen(project);
    navigate(`/grid?db=${encodeURIComponent(project.db_id)}&project=${project.id}`);
  };

  const handleSelectTarget = (target: WithDb<TargetOverview>) => {
    const project = projects.find(
      (candidate) =>
        candidate.db_id === target.db_id && candidate.id === target.project_id
    );
    if (project) markProjectSeen(project);
    navigate(
      `/grid?db=${encodeURIComponent(target.db_id)}&project=${target.project_id}&target=${target.id}`
    );
  };

  // Project expansion handlers (key by db_id + project_id to avoid collisions).
  const projectKey = (dbId: string, projectId: number) => `${dbId}:${projectId}`;
  const toggleProject = (key: string) => {
    const newExpanded = new Set(expandedProjects);
    if (newExpanded.has(key)) {
      newExpanded.delete(key);
    } else {
      newExpanded.add(key);
    }
    setExpandedProjects(newExpanded);
  };

  const toggleDb = (dbId: string) => {
    const next = new Set(collapsedDbs);
    if (next.has(dbId)) next.delete(dbId);
    else next.add(dbId);
    setCollapsedDbs(next);
  };

  if (statsLoading || projectsLoading || targetsLoading) {
    return <div className="overview-loading">Loading overview...</div>;
  }

  if (!databases || databases.length === 0) {
    const managementAllowed = serverInfo?.allow_database_management ?? false;
    return (
      <div className="overview-empty">
        <h2>No databases configured</h2>
        {managementAllowed ? (
          <>
            <p>Add a N.I.N.A. scheduler database to get started.</p>
            <button
              className="action-button primary"
              onClick={() => window.dispatchEvent(new CustomEvent('psf-guard:open-settings'))}
            >
              Open Settings
            </button>
          </>
        ) : (
          <>
            <p>
              This server doesn't permit configuration changes from the
              browser. Register a database on the command line:
            </p>
            <pre className="code-block">
              psf-guard server &lt;db.sqlite&gt; &lt;image-dir&gt;
            </pre>
            <p>
              …or restart with{' '}
              <code>--allow-database-management</code> to enable in-browser
              settings.
            </p>
          </>
        )}
      </div>
    );
  }

  return (
    <div className="overview">
      {/* Overall Statistics */}
      {overallStats && (
        <section className="overview-summary" aria-label="Catalog summary">
          <div className="summary-lead">
            <span>Catalog</span>
            <strong>{overallStats.total_images.toLocaleString()} images</strong>
          </div>
          <dl className="summary-metrics">
            <div>
              <dt>Projects</dt>
              <dd>
                {overallStats.active_projects}
                <span> / {overallStats.total_projects}</span>
              </dd>
            </div>
            <div>
              <dt>Targets</dt>
              <dd>
                {overallStats.active_targets}
                <span> / {overallStats.total_targets}</span>
              </dd>
            </div>
            <div>
              <dt>Accepted</dt>
              <dd>{overallStats.accepted_images.toLocaleString()}</dd>
            </div>
            <div className={overallStats.pending_images > 0 ? 'summary-needs-review' : ''}>
              <dt>To review</dt>
              <dd>{overallStats.pending_images.toLocaleString()}</dd>
            </div>
            <div className={overallStats.files_missing > 0 ? 'summary-has-warning' : ''}>
              <dt>Files</dt>
              <dd>
                {overallStats.files_missing > 0
                  ? `${overallStats.files_missing} missing`
                  : 'All found'}
              </dd>
            </div>
          </dl>
          <div className="summary-grading">
            <div className="summary-progress-label">
              <span>Grading</span>
              <span>
                {getGradingProgress(
                  overallStats.accepted_images,
                  overallStats.rejected_images,
                  overallStats.pending_images
                ).acceptedPct}% accepted
              </span>
            </div>
            <div
              className="summary-progress-bar"
              aria-label={`${overallStats.accepted_images} accepted, ${overallStats.rejected_images} rejected, ${overallStats.pending_images} pending`}
            >
              <div 
                className="progress-accepted" 
                style={{ 
                  width: `${getGradingProgress(
                    overallStats.accepted_images, 
                    overallStats.rejected_images, 
                    overallStats.pending_images
                  ).acceptedPct}%` 
                }}
              />
              <div 
                className="progress-rejected" 
                style={{ 
                  width: `${getGradingProgress(
                    overallStats.accepted_images, 
                    overallStats.rejected_images, 
                    overallStats.pending_images
                  ).rejectedPct}%` 
                }}
              />
              <div 
                className="progress-pending" 
                style={{ 
                  width: `${getGradingProgress(
                    overallStats.accepted_images, 
                    overallStats.rejected_images, 
                    overallStats.pending_images
                  ).pendingPct}%` 
                }}
              />
            </div>
          </div>
        </section>
      )}

      <div className="content-grid">
        {/* Projects grouped by database. Each section is collapsible. */}
        <div className="projects-section">
          {databases.map((db) => {
            const dbProjects = projectsByDb[db.id] || [];
            const isCollapsed = collapsedDbs.has(db.id);
            return (
              <section key={db.id} className="db-section">
                <div className="db-section-heading">
                  <button
                    type="button"
                    className="db-section-toggle"
                    onClick={() => toggleDb(db.id)}
                    aria-expanded={!isCollapsed}
                  >
                    <span
                      className={`expand-toggle ${isCollapsed ? '' : 'expanded'}`}
                      aria-hidden="true"
                    >
                      ▶
                    </span>
                    <span className="db-section-name">{db.name}</span>
                    <span className="db-section-count">
                      {dbProjects.length} project{dbProjects.length === 1 ? '' : 's'}
                    </span>
                  </button>
                  <code className="db-section-slug" title="Database ID">{db.id}</code>
                </div>
                {!isCollapsed && dbProjects.length === 0 && (
                  <div className="empty-state">No projects with images in this database yet.</div>
                )}
                {!isCollapsed && (
                <div className="projects-list">
            {dbProjects.map((project) => {
              const progress = getGradingProgress(
                project.accepted_images,
                project.rejected_images,
                project.pending_images
              );
              const key = projectKey(project.db_id, project.id);
              const projectTargets = targetsByProject[key] || [];
              const isExpanded = expandedProjects.has(key);
              const projectNewImages = newImageCount(project, seenProjects);

              return (
                <div
                  key={key}
                  className={[
                    'project-card',
                    !project.has_files ? 'no-files' : '',
                    projectNewImages > 0 ? 'has-new-images' : '',
                  ].filter(Boolean).join(' ')}
                >
                  <div className="project-header">
                    <button
                      type="button"
                      className="project-open-main"
                      onClick={() => project.has_files && handleSelectProject(project)}
                      disabled={!project.has_files}
                      aria-label={`Open ${project.display_name} images`}
                    >
                      <span className="project-title">{project.display_name}</span>
                      {projectNewImages > 0 && (
                        <span className="new-images-badge">
                          <span aria-hidden="true" />
                          {projectNewImages} new
                        </span>
                      )}
                      <span className="project-open-arrow" aria-hidden="true">→</span>
                    </button>
                    <div className="project-header-actions">
                      {!project.has_files && <span className="no-files-badge">No Files</span>}
                      {organizeAllowed && (
                        <button
                          className="organize-button"
                          title="Rename or merge this project"
                          onClick={(e) => {
                            e.stopPropagation();
                            setOrganizeError('');
                            setOrganizing({
                              kind: 'project',
                              dbId: project.db_id,
                              id: project.id,
                              name: project.name,
                              mergeInto: '',
                            });
                          }}
                        >
                          ✏️
                        </button>
                      )}
                      {projectTargets.length > 0 && (
                        <button
                          type="button"
                          className="project-target-toggle"
                          onClick={() => toggleProject(key)}
                          aria-expanded={isExpanded}
                          aria-label={`${isExpanded ? 'Hide' : 'Show'} targets for ${project.display_name}`}
                          aria-controls={`project-targets-${project.db_id}-${project.id}`}
                        >
                          {projectTargets.length} target
                          {projectTargets.length === 1 ? '' : 's'}
                          <span
                            className={`expand-toggle ${isExpanded ? 'expanded' : ''}`}
                            aria-hidden="true"
                          >
                            ▶
                          </span>
                        </button>
                      )}
                    </div>
                  </div>

                  {organizing?.kind === 'project' &&
                    organizing.dbId === project.db_id &&
                    organizing.id === project.id && (
                      <div className="organize-panel" onClick={(e) => e.stopPropagation()}>
                        <input
                          className="organize-input"
                          value={organizing.name}
                          onChange={(e) => setOrganizing({ ...organizing, name: e.target.value })}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter') saveOrganize();
                            if (e.key === 'Escape') setOrganizing(null);
                          }}
                          placeholder="Project name"
                          autoFocus
                        />
                        <select
                          className="organize-select"
                          value={organizing.mergeInto}
                          onChange={(e) =>
                            setOrganizing({ ...organizing, mergeInto: e.target.value })
                          }
                          title="Merge this project's targets and images into another project"
                        >
                          <option value="">(no merge)</option>
                          {dbProjects
                            .filter((p) => p.id !== project.id)
                            .map((p) => (
                              <option key={p.id} value={p.id}>
                                Merge into: {p.display_name}
                              </option>
                            ))}
                        </select>
                        <button
                          className="organize-save"
                          onClick={saveOrganize}
                          disabled={organizeBusy}
                        >
                          {organizing.mergeInto !== '' ? 'Merge' : 'Save'}
                        </button>
                        <button
                          className="organize-cancel"
                          onClick={() => setOrganizing(null)}
                          disabled={organizeBusy}
                        >
                          Cancel
                        </button>
                        {organizeError && <span className="organize-error">{organizeError}</span>}
                      </div>
                    )}

                  {project.description && (
                    <p className="project-description">{project.description}</p>
                  )}
                  
                  <div className="project-stats">
                    <div className="stat-row">
                      <span>{project.total_images} images</span>
                      {project.total_desired > 0 && (
                        <>
                          <span>{project.accepted_images} / {project.total_desired} desired</span>
                          <span className="completion-badge">
                            {getDesiredProgress(project.accepted_images, project.total_desired)}% complete
                          </span>
                        </>
                      )}
                    </div>
                    <div className="stat-row">
                      <span>{project.accepted_images} accepted</span>
                      <span>{project.rejected_images} rejected</span>
                      <span>{project.pending_images} pending</span>
                    </div>
                    <div className="stat-row">
                      <span>{project.files_found} files found</span>
                      {project.files_missing > 0 && (
                        <span className="files-missing">{project.files_missing} missing</span>
                      )}
                    </div>
                  </div>
                  
                  {/* Desired Progress Bar */}
                  {project.total_desired > 0 && (
                    <div className="project-desired-progress">
                      <div className="progress-label">Desired Progress:</div>
                      <div className="desired-progress-bar">
                        <div
                          className="desired-progress-fill"
                          style={{ width: `${getDesiredProgress(project.accepted_images, project.total_desired)}%` }}
                        />
                      </div>
                    </div>
                  )}
                  
                  {/* Grading Progress Bar */}
                  <div className="project-mini-progress">
                    <div className="progress-label">Grading Progress:</div>
                    <div 
                      className="mini-progress-accepted"
                      style={{ width: `${progress.acceptedPct}%` }}
                    />
                    <div 
                      className="mini-progress-rejected"
                      style={{ width: `${progress.rejectedPct}%` }}
                    />
                    <div 
                      className="mini-progress-pending"
                      style={{ width: `${progress.pendingPct}%` }}
                    />
                  </div>
                  
                  <div className="project-meta">
                    <span>{formatDateRange(project.date_range)}</span>
                    {project.filters_used.length > 0 && (
                      <span>{project.filters_used.join(', ')}</span>
                    )}
                    <button
                      type="button"
                      className="project-meta-link"
                      onClick={(e) => {
                        e.stopPropagation();
                        setSchedulerProject({
                          dbId: project.db_id,
                          id: project.id,
                          name: project.display_name,
                        });
                      }}
                    >
                      Plan &amp; coordinates
                    </button>
                    {project.has_files && project.accepted_images > 0 && (
                      isTauri ? (
                        <span
                          className="export-link"
                          title="Export this project's accepted lights to a local folder (hardlink or copy, rejects excluded)"
                          onClick={(e) => {
                            e.stopPropagation();
                            if (!exportBusy) {
                              handleLocalExport(
                                project.db_id,
                                { project_id: project.id },
                                project.display_name
                              );
                            }
                          }}
                        >
                          ⬇ Export
                        </span>
                      ) : (
                        <a
                          className="export-link"
                          href={apiClient.exportDownloadUrl(project.db_id, {
                            project_id: project.id,
                          })}
                          title="Download this project's accepted lights as a zip (WBPP-style layout, rejects excluded)"
                          onClick={(e) => e.stopPropagation()}
                        >
                          ⬇ Export
                        </a>
                      )
                    )}
                  </div>

                  {/* Nested Targets */}
                  {projectTargets.length > 0 && (
                    <div
                      id={`project-targets-${project.db_id}-${project.id}`}
                      className={`targets-nested ${!isExpanded ? 'collapsed' : ''}`}
                    >
                      {projectTargets.map((target) => {
                        const targetProgress = getGradingProgress(
                          target.accepted_count,
                          target.rejected_count,
                          target.pending_count
                        );
                        
                        return (
                          <div 
                            key={target.id} 
                            className={`target-card ${!target.has_files ? 'no-files' : ''}`}
                            onClick={() => target.has_files && handleSelectTarget(target)}
                          >
                            <div className="target-header">
                              <h4>{target.name}</h4>
                              <div>
                                {!target.has_files && <span className="no-files-badge">No Files</span>}
                                <span className={target.active ? 'active' : 'inactive'}>
                                  {target.active ? 'Active' : 'Inactive'}
                                </span>
                                {organizeAllowed && (
                                  <button
                                    className="organize-button"
                                    title="Rename this target or move it to another project"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      setOrganizeError('');
                                      setOrganizing({
                                        kind: 'target',
                                        dbId: target.db_id,
                                        id: target.id,
                                        name: target.name,
                                        moveTo: '',
                                      });
                                    }}
                                  >
                                    ✏️
                                  </button>
                                )}
                              </div>
                            </div>

                            {organizing?.kind === 'target' &&
                              organizing.dbId === target.db_id &&
                              organizing.id === target.id && (
                                <div
                                  className="organize-panel"
                                  onClick={(e) => e.stopPropagation()}
                                >
                                  <input
                                    className="organize-input"
                                    value={organizing.name}
                                    onChange={(e) =>
                                      setOrganizing({ ...organizing, name: e.target.value })
                                    }
                                    onKeyDown={(e) => {
                                      if (e.key === 'Enter') saveOrganize();
                                      if (e.key === 'Escape') setOrganizing(null);
                                    }}
                                    placeholder="Target name"
                                    autoFocus
                                  />
                                  <select
                                    className="organize-select"
                                    value={organizing.moveTo}
                                    onChange={(e) =>
                                      setOrganizing({ ...organizing, moveTo: e.target.value })
                                    }
                                    title="Move this target (and its images) to another project"
                                  >
                                    <option value="">(keep project)</option>
                                    {dbProjects
                                      .filter((p) => p.id !== target.project_id)
                                      .map((p) => (
                                        <option key={p.id} value={p.id}>
                                          Move to: {p.display_name}
                                        </option>
                                      ))}
                                  </select>
                                  <button
                                    className="organize-save"
                                    onClick={saveOrganize}
                                    disabled={organizeBusy}
                                  >
                                    Save
                                  </button>
                                  <button
                                    className="organize-cancel"
                                    onClick={() => setOrganizing(null)}
                                    disabled={organizeBusy}
                                  >
                                    Cancel
                                  </button>
                                  {organizeError && (
                                    <span className="organize-error">{organizeError}</span>
                                  )}
                                </div>
                              )}

                            {target.coordinates_display && (
                              <p className="target-coordinates">{target.coordinates_display}</p>
                            )}
                            
                            <div className="target-stats">
                              <div className="target-stat-row">
                                <span>{target.image_count} images</span>
                                <span>{target.accepted_count} / {target.total_desired} desired</span>
                                <span className="completion-badge">
                                  {getDesiredProgress(target.accepted_count, target.total_desired)}% complete
                                </span>
                              </div>
                              <div className="target-stat-row">
                                <span>{target.accepted_count} accepted</span>
                                <span>{target.rejected_count} rejected</span>
                                <span>{target.pending_count} pending</span>
                              </div>
                              <div className="target-stat-row">
                                <span>{target.files_found} files found</span>
                                {target.files_missing > 0 && (
                                  <span className="files-missing">{target.files_missing} missing</span>
                                )}
                              </div>
                            </div>
                            
                            <div className="target-mini-progress">
                              <div 
                                className="mini-progress-accepted"
                                style={{ width: `${targetProgress.acceptedPct}%` }}
                              />
                              <div 
                                className="mini-progress-rejected"
                                style={{ width: `${targetProgress.rejectedPct}%` }}
                              />
                              <div 
                                className="mini-progress-pending"
                                style={{ width: `${targetProgress.pendingPct}%` }}
                              />
                            </div>
                            
                            <div className="target-meta">
                              <span>{formatDateRange(target.date_range)}</span>
                              {target.filters_used.length > 0 && (
                                <span>{target.filters_used.join(', ')}</span>
                              )}
                              {target.has_files && target.accepted_count > 0 && (
                                isTauri ? (
                                  <span
                                    className="export-link"
                                    title="Export this target's accepted lights to a local folder (hardlink or copy, rejects excluded)"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      if (!exportBusy) {
                                        handleLocalExport(
                                          target.db_id,
                                          { target_id: target.id },
                                          target.name
                                        );
                                      }
                                    }}
                                  >
                                    ⬇ Export
                                  </span>
                                ) : (
                                  <a
                                    className="export-link"
                                    href={apiClient.exportDownloadUrl(target.db_id, {
                                      target_id: target.id,
                                    })}
                                    title="Download this target's accepted lights as a zip (rejects excluded)"
                                    onClick={(e) => e.stopPropagation()}
                                  >
                                    ⬇ Export
                                  </a>
                                )
                              )}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })}
                </div>
                )}
              </section>
            );
          })}
        </div>
      </div>

      {schedulerProject && (
        <ProjectSchedulerDialog
          open
          dbId={schedulerProject.dbId}
          projectId={schedulerProject.id}
          projectName={schedulerProject.name}
          canEdit={organizeAllowed}
          onClose={() => setSchedulerProject(null)}
        />
      )}

      {/* Footer with GitHub and License Info */}
      <div className="overview-footer">
        <div className="footer-content">
          <p>
            PSF Guard is open source software available on{' '}
            <a 
              href="https://github.com/theatrus/psf-guard" 
              target="_blank" 
              rel="noopener noreferrer"
              className="github-link"
            >
              GitHub
            </a>
          </p>
          <p className="license-info">
            Licensed under the Apache License 2.0
          </p>
        </div>
      </div>
    </div>
  );
}
