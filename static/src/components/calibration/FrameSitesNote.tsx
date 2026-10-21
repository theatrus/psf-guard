import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { frameSitesKey } from './frameSitesKey';

/**
 * Pause in the job line holds reading frame sites: say how long, and offer
 * to resume it now, as flat measuring's note does.
 */
export default function FrameSitesNote({ dbId }: { dbId: string }) {
  const access = useAccess();
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: frameSitesKey(dbId),
    queryFn: () => apiClient.getFrameSitesStatus(dbId),
    refetchInterval: (query) => (query.state.data?.state === 'paused' ? 10_000 : 30_000),
    retry: false,
  });
  const resume = useMutation({
    mutationFn: () => apiClient.resumeJob(dbId, 'sites'),
    onSettled: () => queryClient.invalidateQueries({ queryKey: frameSitesKey(dbId) }),
  });
  if (status.data?.state !== 'paused') return null;
  return (
    <p className="calibration-muted flat-measure-paused" data-testid="frame-sites-paused">
      Reading frame sites is paused until you resume it or the server restarts. Until then, frames
      whose site is not read split their nights where their rig usually is.
      {access.canWrite && (
        <>
          {' '}
          <button type="button" className="link-button" data-testid="frame-sites-resume"
            disabled={resume.isPending} onClick={() => resume.mutate()}>
            Resume
          </button>
        </>
      )}
    </p>
  );
}
