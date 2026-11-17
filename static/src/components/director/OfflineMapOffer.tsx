import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DirectorSurvey } from '../../api/directorTypes';
import type { SkyMapsView } from '../../api/types';
import { openSettings } from '../../utils/settingsIntent';
import { gigabytes, skyMapsKey } from '../skyMapsShared';

const ASKED_KEY = 'psf-guard.framing.offlineMapOffer';
/** The set offered: the whole sky, which every target needs. */
const OFFERED_SET = 'full';

function asked(): boolean {
  try { return window.localStorage.getItem(ASKED_KEY) !== null; } catch { return true; }
}

function remember(answer: 'installed' | 'later') {
  try { window.localStorage.setItem(ASKED_KEY, answer); } catch { /* The offer shows again next time. */ }
}

/**
 * Offers the whole-sky offline map once, the first time Framing opens with
 * none installed. Nothing downloads until Install is clicked; Not now is
 * remembered in this browser, and Settings still lists the maps.
 */
export default function OfflineMapOffer({ surveys, canWrite }: {
  /** Framing's layers, once they are known. */
  surveys: DirectorSurvey[] | undefined;
  canWrite: boolean;
}) {
  const queryClient = useQueryClient();
  const [answered, setAnswered] = useState(asked);
  const [started, setStarted] = useState(false);
  const none = !!surveys && !surveys.some(survey => survey.offline);
  const offering = canWrite && none && !answered;
  // Asked only while the offer or its install shows.
  const maps = useQuery({
    queryKey: skyMapsKey,
    queryFn: apiClient.getSkyMaps,
    enabled: offering || started,
    refetchOnWindowFocus: false,
    refetchInterval: (query) => query.state.data?.install.running ? 2000 : false,
  });
  const install = useMutation({
    mutationFn: () => apiClient.installSkyMap(OFFERED_SET),
    onSuccess: (progress) => {
      // The server answers with the install under way: when another set is
      // installing, this click started nothing. Its answer replaces what
      // the page knew, so an earlier failure does not show again.
      queryClient.setQueryData<SkyMapsView>(skyMapsKey, (current) =>
        current ? { ...current, install: progress } : current);
      if (progress.set === OFFERED_SET) setStarted(true);
      return queryClient.invalidateQueries({ queryKey: skyMapsKey });
    },
  });

  const progress = maps.data?.install;
  const ours = progress?.set === OFFERED_SET;
  // The offer is answered only once the whole-sky map is in place.
  const finished = started && ours && !!progress && !progress.running && !progress.error;
  useEffect(() => {
    if (!finished) return;
    remember('installed');
    void queryClient.invalidateQueries({ queryKey: ['directorSurveys'] });
  }, [finished, queryClient]);

  if (!canWrite || !none) return null;
  // The whole-sky map installing shows as such, however this page came to
  // it: from this offer, Settings, or before the page was reloaded.
  if (ours && progress?.running) {
    const part = progress.bytes_total && progress.bytes_completed !== undefined
      ? ` ${gigabytes(progress.bytes_completed)} of ${gigabytes(progress.bytes_total)}.` : '';
    return <p className="framing-offline-offer" data-testid="offline-map-offer">
      Installing the whole-sky map.{part} You can keep working.
    </p>;
  }
  if (started && ours && progress) {
    if (progress.running) {
      const part = progress.bytes_total && progress.bytes_completed !== undefined
        ? ` ${gigabytes(progress.bytes_completed)} of ${gigabytes(progress.bytes_total)}.` : '';
      return <p className="framing-offline-offer" data-testid="offline-map-offer">
        Installing the whole-sky map.{part} You can keep working.
      </p>;
    }
    if (progress.error) {
      // Not remembered: the offer comes back, so the person can try again.
      return <p className="framing-offline-offer" role="alert" data-testid="offline-map-offer">
        {progress.message}{' '}
        <button type="button" onClick={() => { setStarted(false); install.reset(); }}>Try again</button>{' '}
        <button type="button" onClick={() => openSettings('sky-maps')}>Open Offline sky maps</button>
      </p>;
    }
    return null;
  }
  if (answered) return null;
  const set = maps.data?.sets.find(entry => entry.key === OFFERED_SET);
  const other = progress?.running && !ours
    ? maps.data?.sets.find(entry => entry.key === progress.set)?.name ?? 'Another map'
    : undefined;
  const later = () => { remember('later'); setAnswered(true); };
  // A plain paragraph: the stage's notes are the page's `note`s.
  return <p className="framing-offline-offer" data-testid="offline-map-offer">
    No offline sky map is installed, so the sky is fetched from the survey service as you move.{' '}
    {other
      ? <>{other} is being installed now; install the whole-sky map when it is done.{' '}</>
      : <>Install the whole-sky map{set ? ` (${gigabytes(set.bytes)})` : ''}?{' '}
        <button type="button" disabled={install.isPending} onClick={() => install.mutate()}>
          {install.isPending ? 'Starting…' : 'Install'}
        </button>{' '}</>}
    <button type="button" onClick={later}>Not now</button>{' '}
    <button type="button" onClick={() => openSettings('sky-maps')}>Other maps</button>
    {install.error && <span role="alert"> {install.error.message}</span>}
  </p>;
}
