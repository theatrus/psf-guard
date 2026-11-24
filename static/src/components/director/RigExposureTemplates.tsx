import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Moon, Trash2, Undo2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import type { DirectorMoonPolicy } from '../../api/directorTypes';
import type { ExposureTemplateDetails, UpdateExposureTemplateRequest } from '../../api/types';
import NumberInput from '../NumberInput';
import MoonSettings from './MoonSettings';
import { moonProblem } from './moonPolicy';
import { useUnsavedChanges } from '../../hooks/useUnsavedChanges';
import './TemplateLibrary.css';

const TWILIGHT = ['Nighttime', 'Astronomical', 'Nautical', 'Civil'];

/** A template as the form edits it: the Moon settings in the template
 *  library's shape, so the same fields edit both. */
type Draft = Omit<UpdateExposureTemplateRequest, `moon_${string}`> & { moon: DirectorMoonPolicy };

const cameraValue = (value: number | null) => value == null || value < 0 ? null : value;
const numberOrNull = (text: string) => text.trim() === '' ? null : Number(text);

function draftOf(template: ExposureTemplateDetails): Draft {
  return {
    name: template.name,
    gain: cameraValue(template.gain),
    offset: cameraValue(template.offset),
    bin: template.bin && template.bin > 0 ? template.bin : 1,
    readout_mode: cameraValue(template.readout_mode),
    default_exposure: template.default_exposure,
    twilight_level: template.twilight_level,
    minutes_offset: template.minutes_offset,
    maximum_humidity: template.maximum_humidity,
    dither_every: template.dither_every,
    moon: {
      enabled: template.moon_avoidance_enabled,
      separation_degrees: template.moon_avoidance_separation,
      width_days: template.moon_avoidance_width,
      relax_degrees_per_degree: template.moon_relax_scale,
      relax_min_altitude_degrees: template.moon_relax_min_altitude,
      relax_max_altitude_degrees: template.moon_relax_max_altitude,
      moon_down: template.moon_down_enabled,
    },
  };
}

function requestOf({ moon, name, ...rest }: Draft): UpdateExposureTemplateRequest {
  return {
    ...rest,
    name: name.trim(),
    moon_avoidance_enabled: moon.enabled,
    moon_avoidance_separation: moon.separation_degrees,
    moon_avoidance_width: moon.width_days,
    moon_relax_scale: moon.relax_degrees_per_degree,
    moon_relax_min_altitude: moon.relax_min_altitude_degrees,
    moon_relax_max_altitude: moon.relax_max_altitude_degrees,
    moon_down_enabled: moon.moon_down,
  };
}

/** Why Save is disabled, or null. The server checks the same limits. */
function problemWith(draft: Draft): string | null {
  const whole = (value: number | null) => value == null || (Number.isInteger(value) && value >= 0);
  if (!draft.name.trim()) return 'Enter a template name.';
  if (!whole(draft.gain) || !whole(draft.offset) || !whole(draft.readout_mode)) return 'Gain, offset and readout mode must be whole numbers, or empty for the camera default.';
  if (!Number.isInteger(draft.bin) || draft.bin < 1 || draft.bin > 16) return 'Binning must be between 1 and 16.';
  if (!Number.isFinite(draft.default_exposure) || draft.default_exposure <= 0) return 'The default exposure must be more than 0 seconds.';
  if (!Number.isInteger(draft.minutes_offset) || Math.abs(draft.minutes_offset) > 720) return 'The twilight offset must be a whole number of minutes between -720 and 720.';
  if (!Number.isFinite(draft.maximum_humidity) || draft.maximum_humidity < 0 || draft.maximum_humidity > 100) return 'Maximum humidity must be between 0 and 100 percent.';
  if (!Number.isInteger(draft.dither_every) || draft.dither_every < -1) return 'Dither every must be -1 for the project setting, or 0 or more.';
  // Target Scheduler ignores the Moon fields while avoidance is off.
  return draft.moon.enabled ? moonProblem(draft.moon) : null;
}

/** The drafts with this template's dropped. */
function without(drafts: Record<number, Draft>, id: number) {
  const next = { ...drafts };
  delete next[id];
  return next;
}

const message = (error: unknown) => isAxiosError(error)
  ? error.response?.data?.error || error.message
  : error instanceof Error ? error.message : String(error);

/** The confirmation before Delete: what the deletion reaches. */
function deleteQuestion(template: ExposureTemplateDetails, label: string) {
  return template.syncable === false
    ? `Delete ${label}? It has no GUID, so only this catalog loses it. N.I.N.A. and other catalogs keep their copy.`
    : `Delete ${label}? The next sync also deletes it in other PSF Guard catalogs, and in N.I.N.A. when its Sync plugin supports deleted templates. A copy that a plan uses stays.`;
}

function moonSummary(moon: DirectorMoonPolicy) {
  if (!moon.enabled) return 'Off';
  return moon.moon_down ? 'Moon down' : `${moon.separation_degrees}°`;
}

/**
 * The Target Scheduler exposure templates in a rig's database, each row
 * editable: camera settings, default exposure, twilight, humidity, dither
 * and Moon avoidance. A change applies to every exposure plan that uses the
 * template. The filter stays, because plans and captured frames use it.
 */
export default function RigExposureTemplates({ slug, canEdit }: { slug: string; canEdit: boolean }) {
  const client = useQueryClient();
  const queryKey = ['db', slug, 'exposure-templates'] as const;
  const templates = useQuery({ queryKey, queryFn: () => apiClient.getExposureTemplates(slug) });
  const [drafts, setDrafts] = useState<Record<number, Draft>>({});
  const [expanded, setExpanded] = useState<number | null>(null);
  const [notice, setNotice] = useState('');
  const save = useMutation({
    mutationFn: ({ id, draft }: { id: number; draft: Draft }) => apiClient.updateExposureTemplate(slug, id, requestOf(draft)),
    onSuccess: (saved, sent) => {
      client.setQueryData<ExposureTemplateDetails[]>(queryKey, current => current?.map(row => row.id === saved.id ? saved : row));
      // Edits typed while the save ran stay as a draft.
      setDrafts(current => current[saved.id] === sent.draft ? without(current, saved.id) : current);
      setNotice(`${saved.name} saved`);
      // Project views and plan editors list the same templates.
      return Promise.all([
        client.invalidateQueries({ queryKey: ['db', slug, 'project-scheduler'] }),
        client.invalidateQueries({ queryKey: ['directorTemplates', slug] }),
      ]);
    },
  });
  const remove = useMutation({
    mutationFn: (template: ExposureTemplateDetails) => apiClient.deleteExposureTemplate(slug, template.id),
    onSuccess: removed => {
      client.setQueryData<ExposureTemplateDetails[]>(queryKey, current => current?.filter(row => row.id !== removed.id));
      setDrafts(current => without(current, removed.id));
      setExpanded(current => current === removed.id ? null : current);
      setNotice(removed.synced
        ? `${removed.name} deleted`
        : `${removed.name} deleted here only. It has no GUID, so other catalogs and the rig keep their copy.`);
      return Promise.all([
        client.invalidateQueries({ queryKey: ['db', slug, 'project-scheduler'] }),
        client.invalidateQueries({ queryKey: ['directorTemplates', slug] }),
      ]);
    },
    // A refusal means the list is out of date: a plan may use the template now.
    onError: () => client.invalidateQueries({ queryKey }),
  });

  const rows = templates.data ?? [];
  const profiles = new Set(rows.map(row => row.profile_id));
  const edit = (template: ExposureTemplateDetails, patch: Partial<Draft>) => {
    setNotice('');
    setDrafts(current => ({ ...current, [template.id]: { ...(current[template.id] ?? draftOf(template)), ...patch } }));
  };
  const undo = (id: number) => setDrafts(current => without(current, id));

  const stateOf = (template: ExposureTemplateDetails) => {
    const draft = drafts[template.id] ?? draftOf(template);
    const dirty = template.id in drafts && JSON.stringify(requestOf(drafts[template.id])) !== JSON.stringify(requestOf(draftOf(template)));
    return { draft, dirty, trouble: problemWith(draft), label: template.name || `template #${template.id}` };
  };
  const actions = (template: ExposureTemplateDetails) => {
    const { draft, dirty, trouble, label } = stateOf(template);
    if (!canEdit) return null;
    if (!dirty) {
      const used = template.plan_count > 0;
      return <button type="button" aria-label={`Delete ${label}`}
        title={used ? `${template.plan_count === 1 ? '1 exposure plan uses' : `${template.plan_count} exposure plans use`} this template. Change or delete ${template.plan_count === 1 ? 'that plan' : 'those plans'} first.` : 'Delete'}
        disabled={used || remove.isPending}
        onClick={() => { if (window.confirm(deleteQuestion(template, label))) remove.mutate(template); }}>
        <Trash2 size={16} />
      </button>;
    }
    return <>
      <button type="button" aria-label={`Save ${label}`} title={trouble ?? 'Save'} disabled={!!trouble || save.isPending} onClick={() => save.mutate({ id: template.id, draft })}><Check size={16} /></button>
      <button type="button" aria-label={`Drop changes to ${label}`} title="Drop changes" onClick={() => undo(template.id)}><Undo2 size={16} /></button>
    </>;
  };
  const opened = rows.find(row => row.id === expanded);
  // Why each changed row cannot be saved. The open Moon fields show their own problem.
  const problems = rows.map(row => ({ row, ...stateOf(row) }))
    .filter(entry => entry.dirty && entry.trouble && !(entry.row.id === expanded && entry.trouble === moonProblem(entry.draft.moon)));
  useUnsavedChanges('Target Scheduler templates', rows.some(row => stateOf(row).dirty));

  return <section className="template-library rig-templates" aria-label="Target Scheduler templates">
    <p className="director-muted">The exposure templates in this rig's Target Scheduler database. A change applies to every exposure plan that uses the template. An empty gain, offset or readout mode uses the camera's default. A plan that uses a template from the template library keeps the library's Moon settings when it is activated. You can delete a template that no exposure plan uses.</p>
    {!canEdit && <p className="director-muted">View only. Sign in with an editor's account to change templates.</p>}
    {templates.isPending && <p role="status">Loading exposure templates...</p>}
    {templates.isError && <p className="director-error" role="alert">{message(templates.error)}</p>}
    {templates.data && rows.length === 0 && <p className="director-muted">No exposure templates in this database.</p>}
    {rows.length > 0 && <div className="director-table-scroll"><table className="template-table">
      <thead><tr><th>Name</th><th>Filter</th><th>Gain</th><th>Offset</th><th>Bin</th><th>Readout</th><th>Exposure</th><th>Moon and twilight</th><th></th></tr></thead>
      <tbody>{rows.map(template => {
        const { draft, dirty, label } = stateOf(template);
        const open = expanded === template.id;
        return <tr key={template.id} className={dirty ? 'is-dirty' : undefined}>
          <td><input aria-label={`Name of ${label}`} value={draft.name} maxLength={256} disabled={!canEdit} onChange={event => edit(template, { name: event.target.value })} />
            <small className="director-muted">{template.plan_count === 1 ? '1 plan' : `${template.plan_count} plans`}{profiles.size > 1 ? ` · profile ${template.profile_id}` : ''}</small></td>
          <td>{template.filter_name}</td>
          <td><input aria-label={`Gain of ${label}`} type="number" min={0} step={1} placeholder="–" title="Empty uses the camera default" value={draft.gain ?? ''} disabled={!canEdit} onChange={event => edit(template, { gain: numberOrNull(event.target.value) })} /></td>
          <td><input aria-label={`Offset of ${label}`} type="number" min={0} step={1} placeholder="–" title="Empty uses the camera default" value={draft.offset ?? ''} disabled={!canEdit} onChange={event => edit(template, { offset: numberOrNull(event.target.value) })} /></td>
          <td className="rig-template-bin"><NumberInput aria-label={`Binning of ${label}`} min={1} max={16} step={1} value={draft.bin} disabled={!canEdit} onChange={event => edit(template, { bin: event.target.valueAsNumber })} /></td>
          <td><input aria-label={`Readout mode of ${label}`} type="number" min={0} step={1} placeholder="–" title="Empty uses the camera default" value={draft.readout_mode ?? ''} disabled={!canEdit} onChange={event => edit(template, { readout_mode: numberOrNull(event.target.value) })} /></td>
          <td><span className="plan-goal"><NumberInput aria-label={`Default exposure of ${label}`} min={0} step="any" value={draft.default_exposure} disabled={!canEdit} onChange={event => edit(template, { default_exposure: event.target.valueAsNumber })} /><small>s</small></span></td>
          <td className="template-actions">
            <button type="button" aria-label={`Moon and twilight settings for ${label}`} aria-expanded={open} onClick={() => setExpanded(open ? null : template.id)}>
              <Moon size={16} />{moonSummary(draft.moon)}
            </button>
          </td>
          <td className="template-actions">{actions(template)}</td>
        </tr>;
      })}</tbody>
    </table></div>}
    {opened && (() => {
      const { draft, label } = stateOf(opened);
      return <div className="rig-template-details" role="group" aria-label={`${label}: Moon and twilight`}>
        <h5>{label}: Moon and twilight</h5>
        <fieldset className="template-moon" disabled={!canEdit || save.isPending}>
          <legend>Twilight and conditions</legend>
          <div className="template-moon-fields">
            <label>Twilight
              <span><select aria-label={`Twilight for ${label}`} value={draft.twilight_level} onChange={event => edit(opened, { twilight_level: Number(event.target.value) })}>
                {TWILIGHT.map((name, level) => <option key={name} value={level}>{name}</option>)}
              </select></span>
            </label>
            <label>Twilight offset
              <span><NumberInput aria-label={`Twilight offset for ${label}`} min={-720} max={720} step={1} value={draft.minutes_offset} onChange={event => edit(opened, { minutes_offset: event.target.valueAsNumber })} />min</span>
            </label>
            <label>Maximum humidity
              <span><NumberInput aria-label={`Maximum humidity for ${label}`} min={0} max={100} step="any" value={draft.maximum_humidity} onChange={event => edit(opened, { maximum_humidity: event.target.valueAsNumber })} />% (0 is off)</span>
            </label>
            <label>Dither every
              <span><NumberInput aria-label={`Dither every for ${label}`} min={-1} step={1} value={draft.dither_every} onChange={event => edit(opened, { dither_every: event.target.valueAsNumber })} />frames (-1 uses the project's)</span>
            </label>
          </div>
        </fieldset>
        <MoonSettings value={draft.moon} disabled={!canEdit || save.isPending} onChange={moon => edit(opened, { moon })} />
      </div>;
    })()}
    {problems.map(({ row, label, trouble }) => <p key={row.id} className="director-error" role="alert">{label}: {trouble}</p>)}
    {notice && <p role="status">{notice}</p>}
    {save.isError && <p className="director-error" role="alert">{message(save.error)}</p>}
    {remove.isError && <p className="director-error" role="alert">{message(remove.error)}</p>}
  </section>;
}
