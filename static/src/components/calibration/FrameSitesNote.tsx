import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { frameSitesKey } from './frameSitesKey';

/**
 * Stop in the job line paused reading frame sites: say how it comes back,
 * and offer to take it up now, as flat measuring's note does.
 */
export default function FrameSitesNote({ dbId }: { dbId: string }) {
  const access = useAccess();
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: frameSitesKey(dbId),
    queryFn: () => apiClient.getFrameSitesStatus(dbId),
    refetchInterval: (query) => (query.state.data?.state === 'stopped' ? 10_000 : 30_000),
    retry: false,
  });
  const resume = useMutation({
    mutationFn: () => apiClient.resumeFrameSites(dbId),
    onSuccess: (next) => queryClient.setQueryData(frameSitesKey(dbId), next),
  });
  if (status.data?.state !== 'stopped') return null;
  return (
    <p className="calibration-muted flat-measure-stopped" data-testid="frame-sites-stopped">
      Reading frame sites is stopped. It resumes when frames are imported, or when the server
      starts again; until then frames whose site is not read split their nights where their rig
      usually is.
      {access.canWrite && (
        <>
          {' '}
          <button type="button" className="link-button" data-testid="frame-sites-resume"
            disabled={resume.isPending} onClick={() => resume.mutate()}>
            Resume now
          </button>
        </>
      )}
    </p>
  );
}
