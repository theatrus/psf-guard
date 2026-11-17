import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DirectorSurvey } from '../../api/directorTypes';
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
    refetchInterval: (query) => started && query.state.data?.install.running ? 2000 : false,
  });
  const install = useMutation({
    mutationFn: () => apiClient.installSkyMap(OFFERED_SET),
    onSuccess: () => {
      remember('installed');
      setStarted(true);
      return queryClient.invalidateQueries({ queryKey: skyMapsKey });
    },
  });

  // Installed: Framing reads its layers again and takes the map.
  const finished = started && !!maps.data && !maps.data.install.running && !maps.data.install.error;
  useEffect(() => {
    if (finished) void queryClient.invalidateQueries({ queryKey: ['directorSurveys'] });
  }, [finished, queryClient]);

  if (started) {
    const progress = maps.data?.install;
    if (!progress || progress.running) {
      const part = progress?.bytes_total && progress.bytes_completed !== undefined
        ? ` ${gigabytes(progress.bytes_completed)} of ${gigabytes(progress.bytes_total)}.` : '';
      return <p className="framing-offline-offer" data-testid="offline-map-offer">
        Installing the whole-sky map.{part} You can keep working.
      </p>;
    }
    if (progress.error) {
      return <p className="framing-offline-offer" role="alert" data-testid="offline-map-offer">
        {progress.message}{' '}
        <button type="button" onClick={() => openSettings('sky-maps')}>Open Offline sky maps</button>
      </p>;
    }
    return null;
  }
  if (!offering) return null;
  const set = maps.data?.sets.find(entry => entry.key === OFFERED_SET);
  const later = () => { remember('later'); setAnswered(true); };
  // A plain paragraph: the stage's notes are the page's `note`s.
  return <p className="framing-offline-offer" data-testid="offline-map-offer">
    No offline sky map is installed, so the sky is fetched from the survey service as you move.{' '}
    Install the whole-sky map{set ? ` (${gigabytes(set.bytes)})` : ''}?{' '}
    <button type="button" disabled={install.isPending} onClick={() => install.mutate()}>
      {install.isPending ? 'Starting…' : 'Install'}
    </button>{' '}
    <button type="button" onClick={later}>Not now</button>{' '}
    <button type="button" onClick={() => openSettings('sky-maps')}>Other maps</button>
    {install.error && <span role="alert"> {install.error.message}</span>}
  </p>;
}
