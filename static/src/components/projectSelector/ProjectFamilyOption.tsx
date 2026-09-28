import { projectLastWorkedAt } from '../../utils/projectNavigation';
import { formatRelativeTime } from '../../utils/relativeTime';
import type {
  NavigationFamily,
  NavigationProject,
  NavigationTarget,
} from '../../utils/projectTargetNavigation';
import ProjectTargetRows from './ProjectTargetRows';

interface ProjectFamilyOptionProps {
  family: NavigationFamily;
  targetsFor: (project: NavigationProject) => NavigationTarget[];
  targetsLoading: boolean;
  targetsError: boolean;
  expanded: boolean;
  selectedDbId: string | null;
  selectedProjectId: number | null;
  selectedTargetId: number | null;
  relativeNow: number;
  onToggle: () => void;
  onChooseProject: (project: NavigationProject) => void;
  onChooseTarget: (target: NavigationTarget) => void;
}

/** One project shot by several rigs: a single row naming every database,
 *  which opens to each rig's images and targets. The grid still shows one
 *  database at a time, so every row inside names the rig it opens. */
export default function ProjectFamilyOption({
  family,
  targetsFor,
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
}: ProjectFamilyOptionProps) {
  const latest = family.members
    .map(projectLastWorkedAt)
    .reduce<number | null>((newest, at) => (at !== null && (newest === null || at > newest) ? at : newest), null);
  const rigs = family.members.length;
  return (
    <div className="selector-project-tree selector-family" data-family={family.key}>
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
          {family.display_name}
          <span className="selector-family-badge">{rigs} rigs</span>
        </span>
        <small>
          {family.members.map((member) => member.db_name).join(' · ')}
          {latest !== null ? ` · ${formatRelativeTime(latest, relativeNow)}` : ''}
        </small>
      </button>

      {expanded && (
        <div className="selector-project-targets">
          {family.members.map((member) => (
            <section
              key={`${member.db_id}:${member.id}`}
              className="selector-family-member"
              aria-label={`${family.display_name} on ${member.db_name}`}
            >
              <div className="selector-family-member-heading">
                <span>{member.db_name}</span>
                <span>
                  {member.display_name !== family.display_name ? `${member.display_name} · ` : ''}
                  {(() => {
                    const at = projectLastWorkedAt(member);
                    return at !== null ? formatRelativeTime(at, relativeNow) : 'no images yet';
                  })()}
                </span>
              </div>
              <ProjectTargetRows
                project={member}
                targets={targetsFor(member)}
                targetsLoading={targetsLoading}
                targetsError={targetsError}
                selectedDbId={selectedDbId}
                selectedProjectId={selectedProjectId}
                selectedTargetId={selectedTargetId}
                allImagesCaption={member.db_name}
                onChooseProject={() => onChooseProject(member)}
                onChooseTarget={onChooseTarget}
              />
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
