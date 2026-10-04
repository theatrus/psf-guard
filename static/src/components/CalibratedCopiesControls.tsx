import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';

/** Pairing of calibrated and registered copies other software wrote (WBPP,
 *  Siril, ASTAP, DeepSkyStacker) with the light each came from. On by
 *  default; see docs/design/calibrated-subs.md. */
export default function CalibratedCopiesControls({ dbId, canManage }: { dbId: string; canManage: boolean }) {
  const client = useQueryClient();
  const queryKey = ['db', dbId, 'calibrated-copies'] as const;
  const status = useQuery({ queryKey, queryFn: () => apiClient.getCalibratedCopies(dbId), staleTime: 60_000, retry: false });
  const save = useMutation({
    retry: false,
    mutationFn: (pair: boolean) => apiClient.setPairCalibratedCopies(dbId, pair),
    onSuccess: data => client.setQueryData(queryKey, data),
  });
  const data = status.data;
  if (!data) return null;
  const { calibrated, registered, calibrated_lights: calibratedLights } = data.counts;
  const parts = [
    calibrated > 0 && `${calibrated.toLocaleString()} calibrated`,
    registered > 0 && `${registered.toLocaleString()} registered`,
  ].filter(Boolean);
  return (
    <div className="quality-backfill-option">
      <label title="A calibrated or registered copy of a light is recorded with that light instead of being catalogued as a second frame. A calibrated copy with no raw frame becomes the light until the raw arrives.">
        <input
          type="checkbox"
          checked={data.pair}
          disabled={!canManage || save.isPending}
          onChange={event => save.mutate(event.target.checked)}
        />
        Pair calibrated and registered copies with their lights
      </label>
      <small>
        {parts.length > 0 ? `${parts.join(' and ')} cop${calibrated + registered === 1 ? 'y' : 'ies'} paired.` : 'No copies paired yet.'}
        {calibratedLights > 0 && ` ${calibratedLights.toLocaleString()} light${calibratedLights === 1 ? '' : 's'} come from a calibrated copy and take the raw frame over when it arrives.`}
        {!data.pair && ' Off: copies import as lights of their own.'}
        {!canManage && ' Changing it needs database management on this server.'}
      </small>
      {save.isError && <small className="settings-error" role="alert">{save.error instanceof Error ? save.error.message : 'Saving failed.'}</small>}
    </div>
  );
}
