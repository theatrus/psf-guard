import { projectLastWorkedAt } from '../../utils/projectNavigation';
import { formatRelativeTime } from '../../utils/relativeTime';
import type {
  NavigationProject,
  NavigationTarget,
} from '../../utils/projectTargetNavigation';
import ProjectTargetRows from './ProjectTargetRows';

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
  /** Other databases that hold this same project, when the list keeps each
   *  rig on its own row. */
  alsoOn?: string[];
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
  alsoOn = [],
  onToggle,
  onChooseProject,
  onChooseTarget,
}: ProjectTreeOptionProps) {
  const latest = projectLastWorkedAt(project);

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
          {alsoOn.length > 0 ? ` · also on ${alsoOn.join(', ')}` : ''}
        </small>
      </button>

      {expanded && (
        <div className="selector-project-targets">
          <ProjectTargetRows
            project={project}
            targets={targets}
            targetsLoading={targetsLoading}
            targetsError={targetsError}
            selectedDbId={selectedDbId}
            selectedProjectId={selectedProjectId}
            selectedTargetId={selectedTargetId}
            onChooseProject={onChooseProject}
            onChooseTarget={onChooseTarget}
          />
        </div>
      )}
    </div>
  );
}
