import { useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowLeft } from 'lucide-react';
import { apiClient } from '../api/client';
import type { OrganizationOperation, OrganizationPreview, OrganizationResult } from '../api/types';
import Dialog from './Dialog';
import './OrganizationDialog.css';

export type OrganizationScope = {
  dbId: string;
  sourceTargetId: number;
  sourceTargetName: string;
  sourceProjectId: number;
} & ({ kind: 'merge_targets' } | { kind: 'move_images'; imageIds: number[] } | { kind: 'move_target' });

/** A profile ID as the Library shows it beside a project name. */
const profileTag = (profileId: string) => profileId.slice(0, 8);

const TITLES: Record<OrganizationScope['kind'], string> = {
  merge_targets: 'Merge targets',
  move_images: 'Move exposures',
  move_target: 'Move target',
};

interface Props {
  scope: OrganizationScope;
  onClose: () => void;
  onApplied?: (result: OrganizationResult) => void;
}

export default function OrganizationDialog({ scope, onClose, onApplied }: Props) {
  const queryClient = useQueryClient();
  const merging = scope.kind === 'merge_targets';
  const movingTarget = scope.kind === 'move_target';
  // A moving target needs a project other than its own, so none is chosen
  // at first.
  const [projectId, setProjectId] = useState(movingTarget ? '' : String(scope.sourceProjectId));
  const [targetId, setTargetId] = useState('');
  const [projectName, setProjectName] = useState('');
  const [targetName, setTargetName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [preview, setPreview] = useState<{
    operation: OrganizationOperation;
    summary: OrganizationPreview;
  } | null>(null);
  const destinations = useQuery({
    queryKey: ['db', scope.dbId, 'organization-destinations'],
    queryFn: () => apiClient.getOrganizationDestinations(scope.dbId),
  });
  const sourceProject = destinations.data?.projects.find(project => project.id === scope.sourceProjectId);
  // A whole target can move to a project of any profile; the server refuses
  // a profile change for a target with frames and explains why.
  const compatibleProjects = destinations.data?.projects.filter(
    project => sourceProject && (movingTarget
      ? project.id !== sourceProject.id
      : project.profile_id === sourceProject.profile_id),
  ) ?? [];
  const projectLabel = (project: { name: string; profile_id: string }) =>
    movingTarget && sourceProject && project.profile_id !== sourceProject.profile_id
      ? `${project.name} · profile ${profileTag(project.profile_id)}`
      : project.name;
  const destinationTargets = destinations.data?.targets.filter(
    target => target.project_id === Number(projectId) && target.id !== scope.sourceTargetId,
  ) ?? [];
  const loading = destinations.isPending;
  const loadError = destinations.error;
  const newProject = projectId === 'new';
  const newTarget = !movingTarget && (newProject || targetId === 'new');
  const ready = !loading && !loadError && !!sourceProject
    && (newProject ? !!projectName.trim() : compatibleProjects.some(project => project.id === Number(projectId)))
    && (movingTarget || (newTarget ? !!targetName.trim() : destinationTargets.some(target => target.id === Number(targetId))));

  const change = (update: () => void) => {
    update();
    setPreview(null);
    setError('');
  };
  const makeOperation = (): OrganizationOperation => {
    if (merging) {
      return {
        kind: 'merge_targets',
        source_target_id: scope.sourceTargetId,
        destination_target_id: Number(targetId),
      };
    }
    if (scope.kind === 'move_target') {
      return {
        kind: 'move_target',
        target_id: scope.sourceTargetId,
        destination: newProject
          ? { new_project_name: projectName.trim() }
          : { project_id: Number(projectId) },
      };
    }
    return {
      kind: 'move_images',
      image_ids: scope.imageIds,
      destination: newProject
        ? { new_project_name: projectName.trim(), new_target_name: targetName.trim() }
        : newTarget
          ? { project_id: Number(projectId), new_target_name: targetName.trim() }
          : { target_id: Number(targetId) },
    };
  };
  const previewChange = async () => {
    if (!ready || busy) return;
    setBusy(true);
    setError('');
    setPreview(null);
    try {
      const operation = makeOperation();
      const summary = await apiClient.previewOrganization(scope.dbId, operation);
      setPreview({ operation, summary });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };
  const apply = async () => {
    if (!preview || busy) return;
    setBusy(true);
    setError('');
    try {
      const result = await apiClient.applyOrganization(
        scope.dbId, preview.operation, preview.summary.fingerprint,
      );
      // Cancel old reads before invalidating so an in-flight overview or
      // sequence response cannot replace the new grouping with stale data.
      await queryClient.cancelQueries({ queryKey: ['db', scope.dbId] });
      void queryClient.invalidateQueries({ queryKey: ['db', scope.dbId] });
      onApplied?.(result);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setPreview(null);
    } finally {
      setBusy(false);
    }
  };
  const summary = preview?.summary;

  return (
    <Dialog
      open
      title={TITLES[scope.kind]}
      className="organization-dialog"
      onClose={() => { if (!busy) onClose(); }}
      footer={
        <>
          <button type="button" className="header-button" disabled={busy} onClick={onClose}>Cancel</button>
          {preview && <button
            type="button"
            className="header-button organization-action"
            aria-label="Edit destination"
            disabled={busy}
            onClick={() => setPreview(null)}
          ><ArrowLeft size={14} aria-hidden="true" /> Back</button>}
          <button
            type="button"
            className="action-button"
            disabled={busy || (!preview && !ready)}
            onClick={preview ? apply : previewChange}
          >
            {busy ? (preview ? 'Applying...' : 'Preparing preview...')
              : preview ? (merging ? 'Apply merge' : 'Apply move')
                : merging ? 'Preview merge' : 'Preview move'}
          </button>
        </>
      }
    >
      <div className="organization-source">
        <span>From</span>
        <strong>{summary?.source_target_name ?? scope.sourceTargetName}</strong>
        <span>{summary?.source_project_name ?? sourceProject?.name}</span>
        {scope.kind === 'move_images' && <span>{scope.imageIds.length} selected exposure{scope.imageIds.length === 1 ? '' : 's'}</span>}
      </div>
      {loading && <p role="status">Loading destinations...</p>}
      {loadError && <div role="alert">
        <p>Could not load destinations.</p>
        <button type="button" className="header-button" onClick={() => {
          void destinations.refetch();
        }}>Retry</button>
      </div>}
      {!loading && !loadError && !sourceProject && <p role="alert">The source project is no longer available. Close this dialog and refresh the catalog.</p>}
      {!summary && <fieldset className="organization-fields" disabled={busy || loading || !!loadError}>
        <label>
          Destination project
          <select value={projectId} onChange={event => change(() => {
            setProjectId(event.target.value);
            setTargetId('');
          })}>
            {movingTarget && <option value="">Choose a project</option>}
            {compatibleProjects.map(project => <option key={project.id} value={project.id}>{projectLabel(project)}</option>)}
            {!merging && <option value="new">New project...</option>}
          </select>
        </label>
        {newProject && <label>
          New project name
          <input value={projectName} maxLength={200} onChange={event => change(() => setProjectName(event.target.value))} />
        </label>}
        {!newProject && !movingTarget && <label>
          Destination target
          <select value={targetId} onChange={event => change(() => setTargetId(event.target.value))}>
            <option value="">Choose a target</option>
            {destinationTargets.map(target => <option key={target.id} value={target.id}>{target.name}</option>)}
            {!merging && <option value="new">New target...</option>}
          </select>
        </label>}
        {newTarget && <label>
          New target name
          <input value={targetName} maxLength={200} onChange={event => change(() => setTargetName(event.target.value))} />
        </label>}
      </fieldset>}
      {summary && <section className="organization-preview" aria-label="Change preview" aria-live="polite">
        <h3>Change preview</h3>
        <dl>
          <dt>Destination</dt>
          <dd>{movingTarget
            ? `${summary.destination_project_name}${summary.destination_project_id == null ? ' (new project)' : ''}`
            : `${summary.destination_project_name} / ${summary.destination_target_name}`}</dd>
          <dt>Exposures moved</dt><dd>{summary.images_moved}</dd>
          {summary.exposure_plans_moved > 0 && <><dt>Exposure plans moved</dt><dd>{summary.exposure_plans_moved}</dd></>}
          {summary.exposure_plans_created > 0 && <><dt>Inactive plans created</dt><dd>{summary.exposure_plans_created}</dd></>}
          {summary.profile_change && <>
            <dt>Profile</dt>
            <dd>{profileTag(summary.profile_change.from_profile_id)} to {profileTag(summary.profile_change.to_profile_id)}</dd>
            <dt>Exposure templates reused</dt><dd>{summary.exposure_templates_reused ?? 0}</dd>
            <dt>Exposure templates copied</dt><dd>{summary.exposure_templates_copied ?? 0}</dd>
          </>}
          {summary.planning_panel_id && <><dt>Planning panel</dt><dd>{summary.planning_panel_id}</dd></>}
        </dl>
        {merging && <p>The source target will be removed. The destination keeps its name and framing.</p>}
        {summary.warnings.length > 0 && <ul>{summary.warnings.map(warning => <li key={warning}>{warning}</li>)}</ul>}
      </section>}
      <p className="organization-note">Image files, grades, and capture metadata stay unchanged.</p>
      {error && <p className="organization-error" role="alert">{error}</p>}
    </Dialog>
  );
}
