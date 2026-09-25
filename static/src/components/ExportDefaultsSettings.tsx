import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ExportLayout, WbppOptions } from '../api/types';
import WbppOptionsFields from './WbppOptionsFields';

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
    mutationFn: (update: { default_layout: ExportLayout; wbpp?: WbppOptions }) =>
      apiClient.updateExportSettings(update),
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
      <p className="review-preferences-note">
        What every export starts from. Each export still offers all of these.
      </p>
      <fieldset
        className="calibration-settings-group calibration-matching-group"
        disabled={save.isPending}
      >
        <legend>Layout</legend>
        <label className="review-preference">
          <span>
            Default layout
            <small>
              Grouped by target is PSF Guard&apos;s own tree; WBPP gives each frame type one
              root for PixInsight&apos;s WeightedBatchPreprocessing and writes its runner
              scripts.
            </small>
          </span>
          <select
            value={current.default_layout}
            aria-label="Default export layout"
            onChange={(event) =>
              save.mutate({ default_layout: event.target.value as ExportLayout })
            }
          >
            <option value="standard">Grouped by target</option>
            <option value="wbpp">WBPP</option>
          </select>
        </label>
      </fieldset>
      <fieldset
        className="calibration-settings-group calibration-matching-group"
        disabled={save.isPending}
      >
        <legend>WBPP settings</legend>
        <WbppOptionsFields
          value={current.wbpp}
          idPrefix="default-wbpp"
          onChange={(wbpp) => save.mutate({ default_layout: current.default_layout, wbpp })}
        />
      </fieldset>
      {save.isError && <p className="error-text">{(save.error as Error).message}</p>}
    </div>
  );
}
