import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { Pencil } from 'lucide-react';
import type { DirectorPlanRow } from '../../api/directorTypes';
import PlanThumbnail from './PlanThumbnail';
import { frames, gradingSplit, percentDone, stage, sumFrames } from './planCardModel';

interface PlanCardProps {
  row: DirectorPlanRow;
  href: string;
  canWrite: boolean;
  editing: boolean;
  onRename: () => void;
  /** The arrow that folds the card back to a row. */
  fold?: ReactNode;
}

/** One plan as an Overview-style project card. The headline counts add up
 *  every rig; the Rigs list underneath gives each rig its own bar, since two
 *  rigs shooting one plan each have their own targets and exposure plans. */
export default function PlanCard({ row, href, canWrite, editing, onRename, fold }: PlanCardProps) {
  const name = row.project.name;
  const progress = row.progress;
  const pct = progress ? percentDone(progress) : 0;
  const split = progress ? gradingSplit(progress) : null;
  const rigs = row.links.length;
  return (
    <div className={`project-card director-plan-card${row.activation ? ' is-activated' : ''}`}>
      <div className="project-header">
        {fold}
        <Link className="project-open-main" to={href} aria-label={`Open ${name}`}>
          <span className="project-title">{name}</span>
          {row.links.map(link => (
            <span key={`${link.catalog_slug}:${link.source_project_guid}`} className="project-database" title={`Rig ${link.rig.name}`}>{link.catalog_name}</span>
          ))}
          {row.activation && <span className="director-stage-badge">Active on {row.activation.rigs} rig{row.activation.rigs === 1 ? '' : 's'}</span>}
          <span className="project-open-label">Open plan<span className="project-open-arrow" aria-hidden="true">→</span></span>
        </Link>
        {canWrite && (
          <div className="project-header-actions">
            <button type="button" disabled={editing} title={`Rename ${name}`} aria-label={`Rename ${name}`} onClick={onRename}><Pencil size={16} /></button>
          </div>
        )}
      </div>

      <div className={`director-plan-body${row.framing?.center ? ' has-thumb' : ''}`}>
        {row.framing?.center && <PlanThumbnail framing={row.framing} name={name} />}
        <div className="director-plan-summary">
          <div className="project-stats">
            <div className="stat-row">
              {progress ? (
                <>
                  <span>{progress.acquired} images</span>
                  <span>{progress.accepted} / {progress.desired} desired</span>
                  {progress.desired > 0 && <span className="completion-badge" data-completion={pct}>{pct}% complete</span>}
                </>
              ) : <span>No frames yet</span>}
            </div>
            {progress && split && (
              <div className="stat-row">
                <span>{progress.accepted} accepted</span>
                <span>{progress.rejected} rejected</span>
                <span>{split.pending} pending</span>
              </div>
            )}
          </div>
          {progress && progress.desired > 0 && (
            <div className="project-desired-progress">
              <div className="progress-label">Desired progress</div>
              <div className="desired-progress-bar"><div className="desired-progress-fill" style={{ width: `${pct}%` }} /></div>
            </div>
          )}
          {progress && split && progress.acquired > 0 && (
            <figure className="project-grading-progress">
              <figcaption className="progress-label">Grading status</figcaption>
              <div className="project-mini-progress" role="img" aria-label={`Grading status: ${progress.accepted} accepted, ${progress.rejected} rejected, ${split.pending} pending`}>
                <div className="mini-progress-accepted" style={{ width: `${split.acceptedPct}%` }} />
                <div className="mini-progress-rejected" style={{ width: `${split.rejectedPct}%` }} />
                <div className="mini-progress-pending" style={{ width: `${split.pendingPct}%` }} />
              </div>
            </figure>
          )}
          <div className="project-meta"><span>{stage(row)}</span></div>
        </div>
      </div>

      {rigs > 0 && (
        <section className="project-targets-compact" aria-label={`Rigs shooting ${name}`}>
          <div className="project-targets-heading">
            <span>Rigs</span>
            <span>{rigs} rig{rigs === 1 ? '' : 's'} · each keeps its own targets and frames</span>
          </div>
          <div className="project-targets-list">
            {row.links.map(link => {
              const sum = sumFrames(link.targets);
              const rigPct = percentDone(sum);
              return (
                <div key={`${link.catalog_slug}:${link.source_project_guid}`} className="target-compact-card director-rig-card">
                  <div className="target-compact-main">
                    <span className="target-compact-title">
                      <strong>{link.catalog_name}</strong>
                      {link.source_name && link.source_name !== name && <span className="target-state">{link.source_name}</span>}
                      {link.source_row_id === null && <span className="target-state">project row missing</span>}
                    </span>
                    <span className="target-compact-stats">
                      {link.targets.length === 0 ? 'no target yet' : (
                        <>
                          {frames(sum)}
                          {link.targets.length > 1 && ` · ${link.targets.length} targets`}
                          {sum.desired > 0 && ` · ${rigPct}%`}
                        </>
                      )}
                    </span>
                    {sum.desired > 0 && <div className="desired-progress-bar director-rig-bar"><div className="desired-progress-fill" style={{ width: `${rigPct}%` }} /></div>}
                    {link.targets.length > 1 && <span className="director-plan-panels">{link.targets.map(target => `${target.name} ${frames(target)}`).join(' · ')}</span>}
                  </div>
                </div>
              );
            })}
          </div>
        </section>
      )}
    </div>
  );
}
