import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import './ProjectExposureGrouping.css';

export default function ProjectExposureGrouping({ dbId, projectId, canManage }: {
  dbId: string;
  projectId: number;
  canManage: boolean;
}) {
  const queryClient = useQueryClient();
  const queryKey = ['db', dbId, 'project', projectId, 'processing-settings'];
  const settings = useQuery({
    queryKey,
    queryFn: ({ signal }) => apiClient.getProjectProcessingSettings(dbId, projectId, signal),
  });
  const save = useMutation({
    mutationFn: (enabled: boolean) => apiClient.updateProjectProcessingSettings(
      dbId, projectId, { split_exposure_groups: enabled }
    ),
    onSuccess: async (result) => {
      queryClient.setQueryData(queryKey, result);
      await queryClient.invalidateQueries({
        predicate: (query) => query.queryKey[0] === 'db' && query.queryKey[1] === dbId
          && (query.queryKey.includes('images') || query.queryKey.includes('all-images')
            || (query.queryKey[2] === 'image' && query.queryKey.length === 4)
            || (query.queryKey[2] === 'project' && query.queryKey[3] === projectId
              && !query.queryKey.includes('processing-settings'))),
      });
    },
  });
  const error = save.error ?? settings.error;
  return (
    <div className="project-exposure-grouping" aria-busy={settings.isPending || save.isPending}>
      <label title={canManage ? 'Project processing setting' : 'Database management access required'}>
        <input
          type="checkbox"
          checked={settings.data?.split_exposure_groups ?? false}
          disabled={!canManage || settings.isPending || settings.isError || save.isPending}
          onChange={(event) => save.mutate(event.target.checked)}
        />
        Separate exposure groups
      </label>
      {(settings.isPending || save.isPending) && (
        <span role="status">{save.isPending ? 'Saving...' : 'Loading...'}</span>
      )}
      {error && <span className="error" role="alert">
        {error instanceof Error ? error.message : 'Exposure grouping could not be saved.'}
      </span>}
    </div>
  );
}
