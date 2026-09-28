import type {
  NavigationProject,
  NavigationTarget,
} from '../../utils/projectTargetNavigation';

interface ProjectTargetRowsProps {
  project: NavigationProject;
  targets: NavigationTarget[];
  targetsLoading: boolean;
  targetsError: boolean;
  selectedDbId: string | null;
  selectedProjectId: number | null;
  selectedTargetId: number | null;
  /** What the "All images" row names underneath; the project by default. */
  allImagesCaption?: string;
  onChooseProject: () => void;
  onChooseTarget: (target: NavigationTarget) => void;
}

/** The rows under one project in the picker: all of its images, then each
 *  target. A project shot by several rigs repeats these once per database. */
export default function ProjectTargetRows({
  project,
  targets,
  targetsLoading,
  targetsError,
  selectedDbId,
  selectedProjectId,
  selectedTargetId,
  allImagesCaption,
  onChooseProject,
  onChooseTarget,
}: ProjectTargetRowsProps) {
  const projectSelected =
    selectedDbId === project.db_id &&
    selectedProjectId === project.id &&
    selectedTargetId === null;
  return (
    <>
      <button
        type="button"
        className={`selector-option selector-target-option ${projectSelected ? 'is-selected' : ''}`}
        aria-current={projectSelected ? 'true' : undefined}
        disabled={!project.has_files}
        onClick={onChooseProject}
      >
        <span>All images</span>
        <small>{allImagesCaption ?? project.display_name}</small>
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
    </>
  );
}
