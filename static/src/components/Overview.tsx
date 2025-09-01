import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from 'react-router-dom';
import { apiClient } from '../api/client';
import { useProjectTarget } from '../hooks/useUrlState';
import type { ProjectOverview, TargetOverview, DateRange } from '../api/types';
import './Overview.css';

export default function Overview() {
  const navigate = useNavigate();
  const { setProjectId, setTargetId } = useProjectTarget();
  const [expandedProjects, setExpandedProjects] = useState<Set<number>>(new Set());

  // Fetch overview data
  const { data: overallStats, isLoading: statsLoading } = useQuery({
    queryKey: ['overall-stats'],
    queryFn: apiClient.getOverallStats,
  });

  const { data: projects = [], isLoading: projectsLoading } = useQuery({
    queryKey: ['projects-overview'],
    queryFn: apiClient.getProjectsOverview,
  });

  const { data: targets = [], isLoading: targetsLoading } = useQuery({
    queryKey: ['targets-overview'], 
    queryFn: apiClient.getTargetsOverview,
  });

  // Group targets by project
  const targetsByProject = targets.reduce((acc, target) => {
    if (!acc[target.project_id]) {
      acc[target.project_id] = [];
    }
    acc[target.project_id].push(target);
    return acc;
  }, {} as Record<number, TargetOverview[]>);

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

  const getRequestedProgress = (accepted: number, requested: number) => {
    if (requested === 0) return 0;
    return Math.round((accepted / requested) * 100);
  };

  // Navigation handlers
  const handleSelectProject = (project: ProjectOverview) => {
    setProjectId(project.id);
    setTargetId(null); // Clear target selection
    navigate('/grid');
  };

  const handleSelectTarget = (target: TargetOverview) => {
    setProjectId(target.project_id);
    setTargetId(target.id);
    navigate('/grid');
  };

  const handleViewAllProjects = () => {
    setProjectId(null); // null = all projects
    setTargetId(null);
    navigate('/grid');
  };

  // Project expansion handlers
  const toggleProject = (projectId: number) => {
    const newExpanded = new Set(expandedProjects);
    if (newExpanded.has(projectId)) {
      newExpanded.delete(projectId);
    } else {
      newExpanded.add(projectId);
    }
    setExpandedProjects(newExpanded);
  };

  if (statsLoading || projectsLoading || targetsLoading) {
    return <div className="overview-loading">Loading overview...</div>;
  }

  return (
    <div className="overview">
      <div className="overview-header">
        <h1>PSF Guard Overview</h1>
        <p>Astronomical image grading system</p>
      </div>

      {/* Overall Statistics */}
      {overallStats && (
        <div className="stats-section">
          <h2>Overall Statistics</h2>
          <div className="stats-grid">
            <div className="stat-card">
              <h3>Projects</h3>
              <div className="stat-main">{overallStats.active_projects}</div>
              <div className="stat-sub">of {overallStats.total_projects} total</div>
            </div>
            <div className="stat-card">
              <h3>Targets</h3>
              <div className="stat-main">{overallStats.active_targets}</div>
              <div className="stat-sub">of {overallStats.total_targets} total</div>
            </div>
            <div className="stat-card">
              <h3>Images</h3>
              <div className="stat-main">{overallStats.total_images.toLocaleString()}</div>
              <div className="stat-sub">{overallStats.accepted_images} accepted of {overallStats.total_requested} requested</div>
            </div>
            <div className="stat-card">
              <h3>Completion</h3>
              <div className="stat-main">{getRequestedProgress(overallStats.accepted_images, overallStats.total_requested)}%</div>
              <div className="stat-sub">{overallStats.accepted_images} / {overallStats.total_requested}</div>
            </div>
            <div className="stat-card">
              <h3>Filters</h3>
              <div className="stat-main">{overallStats.unique_filters.length}</div>
              <div className="stat-sub">{formatDateRange(overallStats.date_range).split(' ').slice(0, 3).join(' ')}</div>
            </div>
          </div>

          {/* Progress bar for overall grading */}
          <div className="overall-progress">
            <h4>Overall Grading Progress</h4>
            <div className="progress-bar">
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
            <div className="progress-legend">
              <span className="legend-accepted">
                {overallStats.accepted_images} Accepted ({getGradingProgress(
                  overallStats.accepted_images, 
                  overallStats.rejected_images, 
                  overallStats.pending_images
                ).acceptedPct}%)
              </span>
              <span className="legend-rejected">
                {overallStats.rejected_images} Rejected ({getGradingProgress(
                  overallStats.accepted_images, 
                  overallStats.rejected_images, 
                  overallStats.pending_images
                ).rejectedPct}%)
              </span>
              <span className="legend-pending">
                {overallStats.pending_images} Pending ({getGradingProgress(
                  overallStats.accepted_images, 
                  overallStats.rejected_images, 
                  overallStats.pending_images
                ).pendingPct}%)
              </span>
            </div>
          </div>

          {/* Quick Actions */}
          <div className="quick-actions">
            <button 
              className="action-button primary" 
              onClick={handleViewAllProjects}
            >
              View All Projects
            </button>
          </div>
        </div>
      )}

      <div className="content-grid">
        {/* Projects List with Nested Targets */}
        <div className="projects-section">
          <h2>Projects ({projects.length})</h2>
          <div className="projects-list">
            {projects.map((project) => {
              const progress = getGradingProgress(
                project.accepted_images, 
                project.rejected_images, 
                project.pending_images
              );
              const projectTargets = targetsByProject[project.id] || [];
              const isExpanded = expandedProjects.has(project.id);
              
              return (
                <div key={project.id} className={`project-card ${!project.has_files ? 'no-files' : ''}`}>
                  <div className="project-header" onClick={() => toggleProject(project.id)}>
                    <div style={{ display: 'flex', alignItems: 'center' }}>
                      <h3>{project.name}</h3>
                      {projectTargets.length > 0 && (
                        <span className="target-count">{projectTargets.length} targets</span>
                      )}
                      <span className={`expand-toggle ${isExpanded ? 'expanded' : ''}`}>▶</span>
                    </div>
                    <div>
                      {!project.has_files && <span className="no-files-badge">No Files</span>}
                    </div>
                  </div>
                  
                  {project.description && (
                    <p className="project-description">{project.description}</p>
                  )}
                  
                  <div className="project-stats">
                    <div className="stat-row">
                      <span>{project.total_images} images</span>
                      <span>{project.accepted_images} / {project.total_requested} requested</span>
                      <span className="completion-badge">
                        {getRequestedProgress(project.accepted_images, project.total_requested)}% complete
                      </span>
                    </div>
                    <div className="stat-row">
                      <span>{project.accepted_images} accepted</span>
                      <span>{project.rejected_images} rejected</span>
                      <span>{project.pending_images} pending</span>
                    </div>
                  </div>
                  
                  {/* Requested Progress Bar */}
                  <div className="project-requested-progress">
                    <div className="progress-label">Request Progress:</div>
                    <div className="requested-progress-bar">
                      <div 
                        className="requested-progress-fill"
                        style={{ width: `${getRequestedProgress(project.accepted_images, project.total_requested)}%` }}
                      />
                    </div>
                  </div>
                  
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
                    {project.has_files && (
                      <span 
                        style={{ color: 'var(--color-primary)', cursor: 'pointer', textDecoration: 'underline' }}
                        onClick={(e) => {
                          e.stopPropagation();
                          handleSelectProject(project);
                        }}
                      >
                        View Project →
                      </span>
                    )}
                  </div>

                  {/* Nested Targets */}
                  {projectTargets.length > 0 && (
                    <div className={`targets-nested ${!isExpanded ? 'collapsed' : ''}`}>
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
                              </div>
                            </div>
                            
                            {target.coordinates_display && (
                              <p className="target-coordinates">{target.coordinates_display}</p>
                            )}
                            
                            <div className="target-stats">
                              <div className="target-stat-row">
                                <span>{target.image_count} images</span>
                                <span>{target.accepted_count} / {target.total_requested} requested</span>
                                <span className="completion-badge">
                                  {getRequestedProgress(target.accepted_count, target.total_requested)}% complete
                                </span>
                              </div>
                              <div className="target-stat-row">
                                <span>{target.accepted_count} accepted</span>
                                <span>{target.rejected_count} rejected</span>
                                <span>{target.pending_count} pending</span>
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
        </div>
      </div>
    </div>
  );
}