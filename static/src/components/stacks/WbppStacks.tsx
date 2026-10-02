import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { LatestStackPreviewGroup } from '../../api/types';

function wbppStacksQueryKey(dbId: string, projectId: number) {
  return ['db', dbId, 'wbpp-stacks', projectId] as const;
}

function treatment(entry: LatestStackPreviewGroup): string {
  const source = entry.wbpp;
  if (!source) return '';
  return [
    source.exposure_seconds != null ? `${source.exposure_seconds} s subs` : '',
    source.drizzle ? 'drizzled' : '',
    source.autocrop ? 'cropped' : '',
  ]
    .filter(Boolean)
    .join(' · ');
}

/**
 * The stacks WBPP made for this project. A run that finishes in PSF Guard is
 * taken in on its own; **Take in the last run** covers one that finished
 * before, or after a restart. PSF Guard composes their color in the color
 * section above, where each appears as a channel marked WBPP.
 */
export default function WbppStacks({
  dbId,
  projectId,
  targetId,
}: {
  dbId: string;
  projectId: number;
  targetId?: number | null;
}) {
  const access = useAccess();
  const queryClient = useQueryClient();
  const stacks = useQuery({
    queryKey: wbppStacksQueryKey(dbId, projectId),
    queryFn: () => apiClient.getWbppStacks(dbId, projectId),
  });
  const importRun = useMutation({
    mutationFn: () => apiClient.importWbppStacks(dbId, projectId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: wbppStacksQueryKey(dbId, projectId) });
      // They are channel sources for color now.
      queryClient.invalidateQueries({ queryKey: ['db', dbId] });
    },
  });
  const entries = (stacks.data?.groups ?? []).filter(
    (entry) => targetId == null || entry.group.target_id === targetId
  );

  return (
    <section className="wbpp-stacks" aria-labelledby="wbpp-stacks-heading">
      <header className="wbpp-stacks-header">
        <div>
          <p className="stack-preview-eyebrow">PixInsight</p>
          <h2 id="wbpp-stacks-heading">WBPP stacks</h2>
          <p className="muted">
            Master lights from WBPP runs. PSF Guard composes their color above, where each shows as a
            channel marked WBPP.
          </p>
        </div>
        {access.canWrite && (
          <button
            type="button"
            className="toolbar-button"
            disabled={importRun.isPending}
            onClick={() => importRun.mutate()}
          >
            {importRun.isPending ? 'Taking in…' : 'Take in the last run'}
          </button>
        )}
      </header>
      {importRun.isError && (
        <p className="error-text" role="alert">{(importRun.error as Error).message}</p>
      )}
      {importRun.data && (
        <p className="muted" role="status">
          {importRun.data.imported.length > 0
            ? `Took in ${importRun.data.imported.length} master${importRun.data.imported.length === 1 ? '' : 's'}.`
            : 'No new masters.'}
          {importRun.data.skipped.length > 0 && ` Skipped: ${importRun.data.skipped.join('; ')}`}
        </p>
      )}
      {stacks.isLoading ? null : entries.length === 0 ? (
        <p className="muted">
          No WBPP stacks yet. Stack this project in WBPP from the Library, and its master lights show
          here when the run ends.
        </p>
      ) : (
        <ul className="wbpp-stack-grid">
          {entries.map((entry) => (
            <li key={entry.job_id} className="wbpp-stack-card">
              {entry.group.preview_url && (
                <img
                  src={entry.group.preview_url}
                  alt={`${entry.group.target_name} ${entry.group.filter_name}, stacked by WBPP`}
                  loading="lazy"
                />
              )}
              <div className="wbpp-stack-meta">
                <strong>
                  {entry.group.target_name} · {entry.group.filter_name}
                </strong>
                <span className="muted">{treatment(entry) || 'WBPP master'}</span>
                <span className="muted wbpp-stack-file" title={entry.wbpp?.output_dir}>
                  {entry.wbpp?.master_file}
                </span>
                {entry.group.fits_url && (
                  <a href={entry.group.fits_url} download>
                    FITS
                  </a>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
