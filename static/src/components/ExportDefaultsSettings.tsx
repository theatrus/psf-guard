import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ExportLayout } from '../api/types';

/**
 * The export default: which layout the export dialog starts from. Server-wide,
 * persisted in the registry; every export still offers both layouts at export
 * time.
 */
export default function ExportDefaultsSettings() {
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ['export-settings'],
    queryFn: apiClient.getExportSettings,
  });

  const save = useMutation({
    mutationFn: (layout: ExportLayout) => apiClient.updateExportSettings(layout),
    onSuccess: (updated) => {
      queryClient.setQueryData(['export-settings'], updated);
    },
  });

  if (settings.isLoading) return null;
  if (settings.isError) {
    return (
      <div className="export-defaults-settings">
        <h3>Export</h3>
        <p className="muted">Could not load export settings.</p>
      </div>
    );
  }

  const current = settings.data!;

  return (
    <div className="export-defaults-settings">
      <h3>Export</h3>
      <label className="review-preference">
        <span>
          Default layout
          <small>
            What the export dialog starts from. Grouped by target is PSF
            Guard&apos;s own tree; WBPP gives each frame type one root for
            PixInsight&apos;s WeightedBatchPreprocessing and writes its runner
            scripts. Every export still offers both.
          </small>
        </span>
        <select
          value={current.default_layout}
          aria-label="Default export layout"
          disabled={save.isPending}
          onChange={(event) => save.mutate(event.target.value as ExportLayout)}
        >
          <option value="standard">Grouped by target</option>
          <option value="wbpp">WBPP</option>
        </select>
      </label>
      {save.isError && <p className="error-text">{(save.error as Error).message}</p>}
    </div>
  );
}
