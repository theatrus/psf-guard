import { useState } from 'react';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { ObservingSettings, SchedulingOverrides } from '../../api/directorPreferences';
import { useDraftSection } from './pageDraftsState';
import SchedulingFields from './SchedulingFields';
import { compactOverrides, describeLimit, describeSource, LIMITS, TS_DEFAULTS } from './schedulingModel';
import { retryWhenBusy } from './retry';

const message = (error: unknown) => error instanceof Error ? error.message : 'Could not save the scheduling limits';
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const same = (left: SchedulingOverrides, right: SchedulingOverrides) => JSON.stringify(compactOverrides(left)) === JSON.stringify(compactOverrides(right));

/** This plan's Target Scheduler scheduling limits: an override for each, or
 *  the default every plan, the rig's site or the rig sets. Activation writes
 *  what each rig resolves to into its Target Scheduler project. */
export default function PlanScheduling({ projectId, rigs }: { projectId: string; rigs: Array<{ id: string; name: string }> }) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const key = ['observingSettings', 'project', projectId] as const;
  const stored = useQuery({ queryKey: key, queryFn: () => apiClient.getObservingSettings('project', projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  // What each rig would use without this plan's overrides, and with them.
  const inherited = useQueries({ queries: rigs.map(rig => ({ queryKey: ['observingEffective', rig.id], queryFn: () => apiClient.getEffectiveObserving(rig.id), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false })) });
  const resolved = useQueries({ queries: rigs.map(rig => ({ queryKey: ['observingEffective', rig.id, projectId], queryFn: () => apiClient.getEffectiveObserving(rig.id, projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false })) });
  // The person's edits and the saved limits they started from. Without
  // edits (null, or edits that match their base), the fields show the saved
  // limits, so a newer copy reaches them. Over edits, the edits stay and
  // other limits saved elsewhere are flagged. A save of the plan's priority,
  // the same record, changes no limit.
  const [edit, setEdit] = useState<{ overrides: SchedulingOverrides; base: SchedulingOverrides } | null>(null);
  const savedLimits = stored.data?.scheduling ?? {};
  const edited = edit !== null && !same(edit.overrides, edit.base);
  const draft = edited ? edit.overrides : savedLimits;
  const changedElsewhere = edited && !same(savedLimits, edit.base);
  const change = (next: SchedulingOverrides) => setEdit(current => ({
    overrides: next,
    base: current !== null && !same(current.overrides, current.base) ? current.base : savedLimits,
  }));
  const unsaved = canWrite && !!stored.data && !same(draft, savedLimits);
  const save = useMutation({
    retry: false,
    mutationFn: (settings: ObservingSettings) => apiClient.saveObservingSettings(settings),
    onSuccess: saved => {
      // Our own save is the copy the draft stands on now, not news from
      // elsewhere; edits typed while it ran stay on top of it.
      const base = saved.scheduling ?? {};
      setEdit(current => current === null || same(current.overrides, base) ? null : { overrides: current.overrides, base });
      client.setQueryData(key, saved);
      void client.invalidateQueries({ queryKey: ['observingEffective'] });
    },
    // Someone saved first: load their copy and keep these edits on top of
    // it, so the next save does not meet the same conflict.
    onError: error => { if (httpStatus(error) === 409) void stored.refetch(); },
  });
  const stale = httpStatus(save.error) === 409;
  // Drop the draft for the saved limits, as they are now.
  const reload = () => { save.reset(); setEdit(null); void stored.refetch(); };
  useDraftSection('scheduling', {
    label: 'Scheduling limits',
    order: 3,
    unsaved,
    save: async () => {
      if (!stored.data) return false;
      // Saving now would undo the other limits without a look at them.
      if (stale || changedElsewhere) return 'the limits changed elsewhere; reload them first';
      try { await save.mutateAsync({ ...stored.data, scheduling: compactOverrides(draft) }); } catch (error) { return message(error); }
      return true;
    },
    discard: () => { setEdit(null); save.reset(); },
  });
  const first = inherited[0]?.data?.scheduling;
  return <section className="plan-scheduling" aria-label="Scheduling limits">
    <p className="director-muted">Empty fields inherit</p>
    {stored.isError && <p className="director-error" role="alert">{message(stored.error)}</p>}
    {stored.data && <SchedulingFields label="This plan's limits" overrides={draft} onChange={change} disabled={!canWrite}
      inherited={first?.values ?? TS_DEFAULTS} inheritedFrom={limit => rigs.length > 1 ? 'per rig, below' : describeSource(first?.sources[limit])} />}
    {(stale || changedElsewhere)
      ? <p className="director-error" role="alert">These limits changed elsewhere. <button type="button" onClick={reload}>Reload</button></p>
      : save.isError && <p className="director-error" role="alert">{message(save.error)}</p>}
    {rigs.length > 0 && <div className="plan-scheduling-rigs">
      <table>
        <caption className="director-muted">Per rig{unsaved ? ' (saved)' : ''}</caption>
        <thead><tr><th scope="col">Limit</th>{rigs.map(rig => <th key={rig.id} scope="col">{rig.name}</th>)}</tr></thead>
        <tbody>{LIMITS.map(spec => <tr key={spec.key}>
          <th scope="row">{spec.label}</th>
          {rigs.map((rig, index) => {
            const scheduling = resolved[index]?.data?.scheduling;
            if (!scheduling) return <td key={rig.id} className="director-muted">…</td>;
            return <td key={rig.id}>{describeLimit(spec, scheduling.values[spec.key])}<br /><small className="director-muted">{describeSource(scheduling.sources[spec.key])}</small></td>;
          })}
        </tr>)}</tbody>
      </table>
    </div>}
  </section>;
}
