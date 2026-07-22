import { useEffect, useState, type FormEvent } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  CreateExposurePlanRequest,
  ExposurePlanDetails,
  ExposureTemplateDetails,
  ProjectSchedulerDetails,
  SchedulerTargetDetails,
} from '../api/types';
import Dialog from './Dialog';
import './ProjectSchedulerDialog.css';

interface Props {
  open: boolean;
  dbId: string;
  projectId: number;
  projectName: string;
  canEdit: boolean;
  onClose: () => void;
}

const PROJECT_STATES = ['Draft', 'Active', 'Inactive', 'Closed'];
const PROJECT_PRIORITIES = ['Low', 'Normal', 'High'];

function formatCoordinates(raHours: number, decDegrees: number) {
  const raH = Math.floor(raHours);
  const raMinutes = (raHours - raH) * 60;
  const raM = Math.floor(raMinutes);
  const raS = (raMinutes - raM) * 60;
  const dec = Math.abs(decDegrees);
  const decD = Math.floor(dec);
  const decMinutes = (dec - decD) * 60;
  const decM = Math.floor(decMinutes);
  const decS = (decMinutes - decM) * 60;
  return `${String(raH).padStart(2, '0')}h ${String(raM).padStart(2, '0')}m ${raS.toFixed(1).padStart(4, '0')}s · ${decDegrees < 0 ? '−' : '+'}${String(decD).padStart(2, '0')}° ${String(decM).padStart(2, '0')}′ ${decS.toFixed(1).padStart(4, '0')}″`;
}

function optionalNumber(value: string): number | undefined {
  return value.trim() === '' ? undefined : Number(value);
}

function templateSetting(label: string, value: number | null) {
  return `${label} ${value == null || value < 0 ? 'default' : value}`;
}

function TemplateSection({ templates }: { templates: ExposureTemplateDetails[] }) {
  return (
    <section className="scheduler-section">
      <div className="scheduler-section-heading">
        <div>
          <h3>Shared exposure templates</h3>
          <span className="scheduler-muted">Profile-wide capture and scheduling settings used by exposure plans.</span>
        </div>
        <span className="scheduler-muted">{templates.length} template{templates.length === 1 ? '' : 's'}</span>
      </div>
      {templates.length > 0 ? (
        <div className="scheduler-table-wrap">
          <table className="scheduler-plans scheduler-templates">
            <thead><tr><th>Name</th><th>Filter</th><th>Default</th><th>Capture</th><th>Limits</th><th>Moon</th><th>Timing</th><th>Plans</th></tr></thead>
            <tbody>{templates.map((template) => (
              <tr key={template.id}>
                <td><strong>{template.name}</strong><small>#{template.id}</small></td>
                <td>{template.filter_name}</td>
                <td>{template.default_exposure}s</td>
                <td>{[
                  templateSetting('G', template.gain),
                  templateSetting('O', template.offset),
                  template.bin != null && `${template.bin}×${template.bin}`,
                  templateSetting('R', template.readout_mode),
                ].filter(Boolean).join(' · ')}</td>
                <td>Twilight {template.twilight_level} · humidity {template.maximum_humidity}</td>
                <td>{template.moon_avoidance_enabled
                  ? `${template.moon_avoidance_separation}° / ${template.moon_avoidance_width}° · relax ${template.moon_relax_scale} (${template.moon_relax_min_altitude}°–${template.moon_relax_max_altitude}°)${template.moon_down_enabled ? ' · down only' : ''}`
                  : 'Avoidance off'}</td>
                <td>{template.dither_every < 0 ? 'Project dither' : `Dither ${template.dither_every}`} · {template.minutes_offset >= 0 ? '+' : ''}{template.minutes_offset} min</td>
                <td>{template.plan_count}</td>
              </tr>
            ))}</tbody>
          </table>
        </div>
      ) : <p className="scheduler-empty">No exposure templates in this profile.</p>}
    </section>
  );
}

function ProjectForm({
  project,
  dbId,
  canEdit,
  reload,
}: {
  project: ProjectSchedulerDetails;
  dbId: string;
  canEdit: boolean;
  reload: () => Promise<unknown>;
}) {
  const [form, setForm] = useState(project);
  const [status, setStatus] = useState('');
  useEffect(() => setForm(project), [project]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    setStatus('Saving…');
    try {
      await apiClient.updateProject(dbId, project.id, {
        name: form.name,
        description: form.description ?? '',
        state: form.state,
        priority: form.priority,
        minimum_time: form.minimum_time,
        minimum_altitude: form.minimum_altitude,
        maximum_altitude: form.maximum_altitude,
        use_custom_horizon: form.use_custom_horizon,
        horizon_offset: form.horizon_offset,
        meridian_window: form.meridian_window,
        filter_switch_frequency: form.filter_switch_frequency,
        dither_every: form.dither_every,
        enable_grader: form.enable_grader,
        is_mosaic: form.is_mosaic,
      });
      await reload();
      setStatus('Saved');
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <form className="scheduler-section" onSubmit={save}>
      <div className="scheduler-section-heading">
        <div>
          <h3>Project</h3>
          <span className="scheduler-muted">Profile {project.profile_id}</span>
        </div>
        {project.created_at && (
          <span className="scheduler-muted">
            Created {new Date(project.created_at * 1000).toLocaleDateString()}
          </span>
        )}
      </div>
      <div className="scheduler-fields scheduler-fields-main">
        <label>
          Name
          <input value={form.name} disabled={!canEdit} onChange={(e) => setForm({ ...form, name: e.target.value })} />
        </label>
        <label>
          State
          <select value={form.state} disabled={!canEdit} onChange={(e) => setForm({ ...form, state: Number(e.target.value) })}>
            {PROJECT_STATES.map((label, value) => <option key={label} value={value}>{label}</option>)}
          </select>
        </label>
        <label>
          Priority
          <select value={form.priority} disabled={!canEdit} onChange={(e) => setForm({ ...form, priority: Number(e.target.value) })}>
            {PROJECT_PRIORITIES.map((label, value) => <option key={label} value={value}>{label}</option>)}
          </select>
        </label>
        <label className="scheduler-wide">
          Description
          <textarea rows={2} value={form.description ?? ''} disabled={!canEdit} onChange={(e) => setForm({ ...form, description: e.target.value })} />
        </label>
      </div>
      <details className="scheduler-options">
        <summary>Scheduling limits</summary>
        <div className="scheduler-fields">
          <label>Minimum time (min)<input type="number" min="0" value={form.minimum_time} disabled={!canEdit} onChange={(e) => setForm({ ...form, minimum_time: e.target.valueAsNumber })} /></label>
          <label>Minimum altitude (°)<input type="number" min="-90" max="90" step="0.1" value={form.minimum_altitude} disabled={!canEdit} onChange={(e) => setForm({ ...form, minimum_altitude: e.target.valueAsNumber })} /></label>
          <label>Maximum altitude (°)<input type="number" min="-90" max="90" step="0.1" value={form.maximum_altitude} disabled={!canEdit} onChange={(e) => setForm({ ...form, maximum_altitude: e.target.valueAsNumber })} /></label>
          <label>Horizon offset (°)<input type="number" step="0.1" value={form.horizon_offset} disabled={!canEdit} onChange={(e) => setForm({ ...form, horizon_offset: e.target.valueAsNumber })} /></label>
          <label>Meridian window<input type="number" value={form.meridian_window} disabled={!canEdit} onChange={(e) => setForm({ ...form, meridian_window: e.target.valueAsNumber })} /></label>
          <label>Filter switch every<input type="number" min="0" value={form.filter_switch_frequency} disabled={!canEdit} onChange={(e) => setForm({ ...form, filter_switch_frequency: e.target.valueAsNumber })} /></label>
          <label>Dither every<input type="number" min="0" value={form.dither_every} disabled={!canEdit} onChange={(e) => setForm({ ...form, dither_every: e.target.valueAsNumber })} /></label>
        </div>
        <div className="scheduler-checks">
          <label><input type="checkbox" checked={form.use_custom_horizon} disabled={!canEdit} onChange={(e) => setForm({ ...form, use_custom_horizon: e.target.checked })} /> Use custom horizon</label>
          <label><input type="checkbox" checked={form.enable_grader} disabled={!canEdit} onChange={(e) => setForm({ ...form, enable_grader: e.target.checked })} /> Enable grader</label>
          <label><input type="checkbox" checked={form.is_mosaic} disabled={!canEdit} onChange={(e) => setForm({ ...form, is_mosaic: e.target.checked })} /> Mosaic project</label>
        </div>
      </details>
      {canEdit && <div className="scheduler-actions"><span role="status">{status}</span><button type="submit">Save project</button></div>}
    </form>
  );
}

function PlanRow({ plan, dbId, canEdit, reload }: { plan: ExposurePlanDetails; dbId: string; canEdit: boolean; reload: () => Promise<unknown> }) {
  const [exposure, setExposure] = useState(plan.exposure);
  const [desired, setDesired] = useState(plan.desired);
  const [enabled, setEnabled] = useState(plan.enabled);
  const [status, setStatus] = useState('');
  useEffect(() => {
    setExposure(plan.exposure);
    setDesired(plan.desired);
    setEnabled(plan.enabled);
  }, [plan]);
  const save = async () => {
    setStatus('Saving…');
    try {
      await apiClient.updateExposurePlan(dbId, plan.id, { exposure, desired, enabled });
      await reload();
      setStatus('Saved');
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    }
  };
  return (
    <tr>
      <td><strong>{plan.filter_name}</strong><small>{plan.template_name}</small></td>
      <td><input aria-label={`${plan.filter_name} exposure seconds`} title="-1 uses the exposure template default" type="number" min="-1" step="0.1" value={exposure} disabled={!canEdit} onChange={(e) => setExposure(e.target.valueAsNumber)} /></td>
      <td><input aria-label={`${plan.filter_name} desired count`} type="number" min="0" value={desired} disabled={!canEdit} onChange={(e) => setDesired(e.target.valueAsNumber)} /></td>
      <td>{plan.acquired}</td>
      <td>{plan.accepted}</td>
      <td><input aria-label={`${plan.filter_name} enabled`} type="checkbox" checked={enabled} disabled={!canEdit} onChange={(e) => setEnabled(e.target.checked)} /></td>
      <td>{[templateSetting('G', plan.gain), templateSetting('O', plan.offset), plan.bin != null && `${plan.bin}×${plan.bin}`, templateSetting('R', plan.readout_mode)].filter(Boolean).join(' · ')}</td>
      {canEdit && <td><button type="button" className="scheduler-small-button" onClick={save}>Save</button><small role="status">{status}</small></td>}
    </tr>
  );
}

const EMPTY_PLAN: CreateExposurePlanRequest = {
  filter_name: '',
  exposure: 60,
  desired: 1,
  bin: 1,
  enabled: true,
};

function NewPlanForm({ target, templates, dbId, reload, onDone }: { target: SchedulerTargetDetails; templates: ExposureTemplateDetails[]; dbId: string; reload: () => Promise<unknown>; onDone: () => void }) {
  const [form, setForm] = useState(EMPTY_PLAN);
  const [status, setStatus] = useState('');
  const selectedTemplate = templates.find((template) => template.id === form.exposure_template_id);
  const chooseTemplate = (value: string) => {
    if (value === '') {
      setForm({
        ...EMPTY_PLAN,
        exposure: form.exposure,
        desired: form.desired,
        enabled: form.enabled,
      });
      return;
    }
    const template = templates.find((candidate) => candidate.id === Number(value));
    if (!template) return;
    setForm({
      ...form,
      exposure_template_id: template.id,
      template_name: template.name,
      filter_name: template.filter_name,
      gain: template.gain ?? undefined,
      offset: template.offset ?? undefined,
      bin: template.bin ?? 1,
      readout_mode: template.readout_mode ?? undefined,
    });
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setStatus('Creating…');
    try {
      await apiClient.createExposurePlan(dbId, target.id, form);
      await reload();
      onDone();
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    }
  };
  return (
    <form className="scheduler-new-plan" onSubmit={submit} noValidate>
      <h5>New exposure plan</h5>
      <div className="scheduler-fields">
        <label className="scheduler-wide">Exposure template<select aria-label="Exposure template" value={form.exposure_template_id ?? ''} onChange={(e) => chooseTemplate(e.target.value)}><option value="">Match settings or create a template</option>{templates.map((template) => <option key={template.id} value={template.id}>{template.name} · {template.filter_name} · {template.default_exposure}s</option>)}</select></label>
        <label>Template name<input value={form.template_name ?? ''} disabled={Boolean(selectedTemplate)} onChange={(e) => setForm({ ...form, template_name: e.target.value || undefined })} /></label>
        <label>Filter<input required value={form.filter_name ?? ''} disabled={Boolean(selectedTemplate)} onChange={(e) => setForm({ ...form, filter_name: e.target.value })} /></label>
        <label>Exposure (s)<input required type="number" min="0.001" step="0.1" value={form.exposure} onChange={(e) => setForm({ ...form, exposure: e.target.valueAsNumber })} /></label>
        <label>Desired<input required type="number" min="0" value={form.desired} onChange={(e) => setForm({ ...form, desired: e.target.valueAsNumber })} /></label>
        <label>Gain<input type="number" value={form.gain ?? ''} disabled={Boolean(selectedTemplate)} onChange={(e) => setForm({ ...form, gain: optionalNumber(e.target.value) })} /></label>
        <label>Offset<input type="number" value={form.offset ?? ''} disabled={Boolean(selectedTemplate)} onChange={(e) => setForm({ ...form, offset: optionalNumber(e.target.value) })} /></label>
        <label>Bin<input required type="number" min="1" value={form.bin ?? 1} disabled={Boolean(selectedTemplate)} onChange={(e) => setForm({ ...form, bin: e.target.valueAsNumber })} /></label>
        <label>Readout mode<input type="number" value={form.readout_mode ?? ''} disabled={Boolean(selectedTemplate)} onChange={(e) => setForm({ ...form, readout_mode: optionalNumber(e.target.value) })} /></label>
        <label className="scheduler-check"><input type="checkbox" checked={form.enabled} onChange={(e) => setForm({ ...form, enabled: e.target.checked })} /> Enabled</label>
      </div>
      <div className="scheduler-actions"><span role="status">{status}</span><button type="button" onClick={onDone}>Cancel</button><button type="submit">Create plan</button></div>
    </form>
  );
}

function TargetSection({ target, templates, dbId, canEdit, reload }: { target: SchedulerTargetDetails; templates: ExposureTemplateDetails[]; dbId: string; canEdit: boolean; reload: () => Promise<unknown> }) {
  const [form, setForm] = useState(target);
  const [adding, setAdding] = useState(false);
  const [status, setStatus] = useState('');
  useEffect(() => setForm(target), [target]);
  const save = async () => {
    setStatus('Saving…');
    try {
      await apiClient.updateTarget(dbId, target.id, {
        name: form.name,
        active: form.active,
        ra_hours: form.ra_hours,
        dec_degrees: form.dec_degrees,
        epoch_code: form.epoch_code,
        rotation: form.rotation,
        roi: form.roi,
      });
      await reload();
      setStatus('Saved');
    } catch (error) {
      setStatus(error instanceof Error ? error.message : String(error));
    }
  };
  return (
    <details className="scheduler-target" open>
      <summary><span>{target.name}</span><span>{formatCoordinates(target.ra_hours, target.dec_degrees)} · {target.exposure_plans.length} plan{target.exposure_plans.length === 1 ? '' : 's'}</span></summary>
      <div className="scheduler-target-body">
        <div className="scheduler-fields scheduler-target-fields">
          <label>Name<input value={form.name} disabled={!canEdit} onChange={(e) => setForm({ ...form, name: e.target.value })} /></label>
          <label>RA (decimal hours)<input type="number" min="0" max="23.999999" step="0.000001" value={form.ra_hours} disabled={!canEdit} onChange={(e) => setForm({ ...form, ra_hours: e.target.valueAsNumber })} /></label>
          <label>Dec (degrees)<input type="number" min="-90" max="90" step="0.000001" value={form.dec_degrees} disabled={!canEdit} onChange={(e) => setForm({ ...form, dec_degrees: e.target.valueAsNumber })} /></label>
          <label>Epoch<select value={form.epoch_code} disabled={!canEdit} onChange={(e) => setForm({ ...form, epoch_code: Number(e.target.value) })}><option value={0}>JNow</option><option value={1}>B1950</option><option value={2}>J2000</option></select></label>
          <label>Rotation (°)<input type="number" step="0.1" value={form.rotation} disabled={!canEdit} onChange={(e) => setForm({ ...form, rotation: e.target.valueAsNumber })} /></label>
          <label>ROI (%)<input type="number" min="0.1" step="0.1" value={form.roi} disabled={!canEdit} onChange={(e) => setForm({ ...form, roi: e.target.valueAsNumber })} /></label>
          <label className="scheduler-check"><input type="checkbox" checked={form.active} disabled={!canEdit} onChange={(e) => setForm({ ...form, active: e.target.checked })} /> Active</label>
        </div>
        {canEdit && <div className="scheduler-actions"><span role="status">{status}</span><button type="button" onClick={save}>Save target</button></div>}
        <div className="scheduler-plans-heading"><div><h4>Exposure plans</h4><span className="scheduler-muted">−1 seconds uses the exposure template default.</span></div>{canEdit && !adding && <button type="button" onClick={() => setAdding(true)}>Add plan</button>}</div>
        {target.exposure_plans.length > 0 ? (
          <div className="scheduler-table-wrap"><table className="scheduler-plans"><thead><tr><th>Filter</th><th>Seconds</th><th>Desired</th><th>Acquired</th><th>Accepted</th><th>On</th><th>Template</th>{canEdit && <th />}</tr></thead><tbody>{target.exposure_plans.map((plan) => <PlanRow key={plan.id} plan={plan} dbId={dbId} canEdit={canEdit} reload={reload} />)}</tbody></table></div>
        ) : <p className="scheduler-empty">No exposure plans.</p>}
        {adding && <NewPlanForm target={target} templates={templates} dbId={dbId} reload={reload} onDone={() => setAdding(false)} />}
      </div>
    </details>
  );
}

export default function ProjectSchedulerDialog({ open, dbId, projectId, projectName, canEdit, onClose }: Props) {
  const queryClient = useQueryClient();
  const queryKey = ['db', dbId, 'project-scheduler', projectId] as const;
  const query = useQuery({
    queryKey,
    queryFn: () => apiClient.getProjectScheduler(dbId, projectId),
    enabled: open,
  });
  const reload = async () => {
    await query.refetch();
    await queryClient.invalidateQueries({ queryKey: ['db', dbId] });
  };
  return (
    <Dialog open={open} title={`Project plan · ${projectName}`} onClose={onClose} className="scheduler-dialog">
      {!canEdit && <p className="scheduler-readonly">View only. Start the server with database management enabled to change scheduler data.</p>}
      {query.isLoading && <p>Loading project plan…</p>}
      {query.error && <p className="scheduler-error">{query.error instanceof Error ? query.error.message : String(query.error)}</p>}
      {query.data && (
        <div className="scheduler-content">
          <ProjectForm project={query.data} dbId={dbId} canEdit={canEdit} reload={reload} />
          <TemplateSection templates={query.data.exposure_templates ?? []} />
          <div className="scheduler-targets-heading"><h3>Targets and coordinates</h3><span>{query.data.targets.length} target{query.data.targets.length === 1 ? '' : 's'}</span></div>
          {query.data.targets.map((target) => <TargetSection key={target.id} target={target} templates={query.data.exposure_templates ?? []} dbId={dbId} canEdit={canEdit} reload={reload} />)}
          {query.data.targets.length === 0 && <p className="scheduler-empty">This project has no targets.</p>}
        </div>
      )}
    </Dialog>
  );
}
