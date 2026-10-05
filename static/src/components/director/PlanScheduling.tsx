import { useEffect, useState } from 'react';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { ObservingSettings, SchedulingOverrides } from '../../api/directorPreferences';
import { useDraftSection } from './pageDraftsState';
import SchedulingFields from './SchedulingFields';
import { compactOverrides, describeLimit, describeSource, LIMITS, TS_DEFAULTS } from './schedulingModel';
import { retryWhenBusy } from './retry';

const message = (error: unknown) => error instanceof Error ? error.message : 'Could not save the scheduling limits';

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
  const [draft, setDraft] = useState<SchedulingOverrides>({});
  useEffect(() => { if (stored.data) setDraft(stored.data.scheduling ?? {}); }, [stored.data]);
  const baseline = compactOverrides(stored.data?.scheduling ?? {});
  const unsaved = canWrite && !!stored.data && JSON.stringify(compactOverrides(draft)) !== JSON.stringify(baseline);
  const save = useMutation({
    retry: false,
    mutationFn: (settings: ObservingSettings) => apiClient.saveObservingSettings(settings),
    onSuccess: saved => {
      client.setQueryData(key, saved);
      void client.invalidateQueries({ queryKey: ['observingEffective'] });
    },
  });
  useDraftSection('scheduling', {
    label: 'Scheduling limits',
    order: 3,
    unsaved,
    save: async () => {
      if (!stored.data) return false;
      try { await save.mutateAsync({ ...stored.data, scheduling: compactOverrides(draft) }); } catch { return false; }
      return true;
    },
    discard: () => { setDraft(stored.data?.scheduling ?? {}); save.reset(); },
  });
  const first = inherited[0]?.data?.scheduling;
  return <section className="plan-scheduling" aria-label="Scheduling limits">
    <p className="director-muted">Target Scheduler's limits for this plan. Leave a field empty to use the default set for every plan, the rig's site or the rig; activation writes the result into each rig's Target Scheduler project.</p>
    {stored.isError && <p className="director-error" role="alert">{message(stored.error)}</p>}
    {stored.data && <SchedulingFields label="This plan's limits" overrides={draft} onChange={setDraft} disabled={!canWrite}
      inherited={first?.values ?? TS_DEFAULTS} inheritedFrom={limit => rigs.length > 1 ? 'per rig, below' : describeSource(first?.sources[limit])} />}
    {save.isError && <p className="director-error" role="alert">{message(save.error)}</p>}
    {rigs.length > 0 && <div className="plan-scheduling-rigs">
      <table>
        <caption className="director-muted">What each rig gets{unsaved ? ', from the saved limits' : ''}</caption>
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
