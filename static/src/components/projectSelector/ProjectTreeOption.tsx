import { projectLastWorkedAt } from '../../utils/projectNavigation';
import { formatRelativeTime } from '../../utils/relativeTime';
import type {
  NavigationProject,
  NavigationTarget,
} from '../../utils/projectTargetNavigation';

interface ProjectTreeOptionProps {
  project: NavigationProject;
  targets: NavigationTarget[];
  targetsLoading: boolean;
  targetsError: boolean;
  expanded: boolean;
  selectedDbId: string | null;
  selectedProjectId: number | null;
  selectedTargetId: number | null;
  relativeNow: number;
  onToggle: () => void;
  onChooseProject: () => void;
  onChooseTarget: (target: NavigationTarget) => void;
}

export default function ProjectTreeOption({
  project,
  targets,
  targetsLoading,
  targetsError,
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
      <button
        type="button"
        className="selector-option selector-project-toggle"
        aria-expanded={expanded}
        onClick={onToggle}
      >
        <span className="selector-project-title">
          <span className={`selector-chevron ${expanded ? 'expanded' : ''}`} aria-hidden="true">
            ▶
          </span>
          {project.display_name}
        </span>
        <small>
          {project.db_name}
          {latest !== null ? ` · ${formatRelativeTime(latest, relativeNow)}` : ''}
          {!project.has_files ? ' · no files' : ''}
        </small>
      </button>

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
                  {target.active ? 'Active target' : 'Inactive target'}
                  {!target.has_files ? ' · no files' : ''}
                </small>
              </button>
            );
          })}

          {targetsLoading && <p className="selector-empty">Loading targets…</p>}
          {!targetsLoading && !targetsError && targets.length === 0 && (
            <p className="selector-empty">No matching targets.</p>
          )}
        </div>
      )}
    </div>
  );
}
