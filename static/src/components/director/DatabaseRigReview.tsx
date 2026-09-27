import { useEffect, useRef, useState } from 'react';
import { Check, Eye } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorRigReport } from '../../api/directorTypes';
import { identityId } from './identityId';
import type { CatalogData } from './catalogData';
import { isAxiosError } from 'axios';

export default function DatabaseRigReview({ data, refreshing, onBusy, onApplied }: { data: CatalogData; refreshing: boolean; onBusy: (busy: boolean) => void; onApplied: () => void }) {
  const { canWrite } = useAccess();
  const [plan] = useState(() => ({ catalog_id: data.discovery.catalog_identity?.id ?? identityId() }));
  const [review, setReview] = useState<DirectorRigReport | null>(null);
  const [busy, setBusy] = useState(false);
  const active = useRef(false);
  const [error, setError] = useState('');
  useEffect(() => { onBusy(busy); return () => onBusy(false); }, [busy, onBusy]);
  const run = async (apply: boolean) => {
    if (active.current || !canWrite || refreshing) return;
    active.current = true; setBusy(true); setError('');
    try {
      if (apply && review) {
        await apiClient.applyDirectorRig(data.discovery.catalog_slug, plan, review.preview_digest);
        onApplied();
      } else setReview(await apiClient.previewDirectorRig(data.discovery.catalog_slug, plan));
    } catch (cause) {
      setError(isAxiosError(cause) ? cause.response?.data?.error || cause.message : cause instanceof Error ? cause.message : 'Database planning request failed');
      const httpError = isAxiosError(cause) ? cause
        : cause instanceof Error && isAxiosError(cause.cause) ? cause.cause : null;
      if (httpError?.response?.status === 409) setReview(null);
    } finally { active.current = false; setBusy(false); }
  };
  return <section aria-label="Database rig setup">
    <h3>{data.discovery.catalog_name}</h3>
    {error && <p role="alert" className="director-error">{error}</p>}
    <p className="director-muted">{review ? 'Ready to enable planning' : 'Planning not enabled'}</p>
    {review && <dl className="director-project-context"><dt>Rig database</dt><dd>{data.discovery.catalog_name}</dd><dt>Identity</dt><dd>{review.binding.rig.id}</dd></dl>}
    {canWrite && <div className="director-actions">
      {review ? <><button type="button" disabled={busy || refreshing} onClick={() => void run(true)}><Check size={16} />{busy ? 'Applying...' : 'Enable planning'}</button>
        <button type="button" disabled={busy} onClick={() => setReview(null)}>Cancel</button></>
        : <button type="button" disabled={busy || refreshing} onClick={() => void run(false)}><Eye size={16} />{busy ? 'Reviewing...' : 'Preview rig setup'}</button>}
    </div>}
  </section>;
}
