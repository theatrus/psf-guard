import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StackColorJob, StackContinuumResult } from '../api/types';
const statuses = { applied: 'Applied', manual: 'Manual', skipped: 'Skipped', disabled: 'Off' };
const labels: Record<string, string> = { ha: 'Hα', oiii: 'OIII', sii: 'SII', red: 'R', green: 'G', blue: 'B' };
function Line({ dbId, jobId, line }: { dbId: string; jobId: string; line: StackContinuumResult }) {
  const [inspect, setInspect] = useState(false);
  const report = useQuery({ queryKey: ['db', dbId, 'continuum-report', jobId, line.role],
    queryFn: () => apiClient.getStackContinuumReport(dbId, jobId, line.role), enabled: inspect, retry: false });
  const diagnostics = report.data?.status === 'fitted' ? report.data.plan?.diagnostics : report.data?.diagnostics;
  const stars = diagnostics?.stars?.filter((star) => star.flux > 0 && star.fractionalResidual !== null) ?? [];
  const fluxes = stars.map((star) => Math.log10(star.flux));
  const low = Math.min(...fluxes), span = Math.max(...fluxes) - low || 1;
  const limit = Math.max(0.2, ...stars.map((star) => Math.abs(star.fractionalResidual ?? 0)));
  return <details>
    <summary>{labels[line.role]} · {statuses[line.status]} · {line.detail}</summary>
    {line.plan && <p>Weights ({line.references.map((r) => labels[r]).join(', ')}): {line.plan.coefficients.map((v) => v.toPrecision(5)).join(', ')}</p>}
    {line.diagnostics && <p>{line.diagnostics.usedStars} training · {line.diagnostics.validationStars} held out · {line.diagnostics.eligibleStars} eligible / {line.diagnostics.candidateStars} candidates
      {line.diagnostics.validationFractionalRms != null && ` · Held-out RMS ${(line.diagnostics.validationFractionalRms * 100).toFixed(1)}%`}</p>}
    {line.status !== 'disabled' && <p>{line.clipping_maps ? 'Producer clipping maps used' : line.status === 'manual' ? 'Manual weights; signed residual retained' : 'Review saturation evidence in the report'}</p>}
    {line.residual_fits_url && <a href={line.residual_fits_url} download>Signed residual FITS</a>}{' '}
    {line.residual_preview_url && <a href={line.residual_preview_url} target="_blank" rel="noreferrer">Residual preview</a>}{' '}
    {line.report_url && <><a href={line.report_url} target="_blank" rel="noreferrer">Full stellar fit report</a>{' '}
      <button type="button" onClick={() => setInspect((value) => !value)} aria-expanded={inspect}>Inspect stellar residuals</button></>}
    {inspect && <div>
      {report.isPending && <p role="status">Loading stellar residuals…</p>}
      {report.error && <p role="alert">{report.error instanceof Error ? report.error.message : 'Stellar report could not be loaded'}</p>}
      {report.data && (stars.length ? <figure>
        <svg viewBox="0 0 420 210" role="img" aria-label={`${labels[line.role]} fractional stellar residual versus log flux`}>
          <path d="M40 15V175H405M40 95H405" stroke="currentColor" fill="none" />
          <text x="42" y="12" fontSize="10">Fractional residual ±{limit.toFixed(2)}</text>
          <text x="145" y="205" fontSize="12">Stellar flux (log scale)</text>
          {stars.map((star, i) => <circle key={i} cx={45 + (Math.log10(star.flux) - low) / span * 350}
            cy={95 - (star.fractionalResidual ?? 0) / limit * 75} r={star.validation ? 3 : 2}
            fill={star.validation ? 'none' : 'currentColor'} stroke="currentColor"><title>{star.validation ? 'Held out' : star.used ? 'Training' : 'Rejected'} · flux {star.flux.toPrecision(4)} · residual {star.fractionalResidual?.toFixed(4)}</title></circle>)}
        </svg><figcaption>Open circles are held-out stars. A trend with brightness can indicate that one continuum scale is unreliable.</figcaption>
      </figure> : <p>No measured stellar residuals are available.</p>)}
    </div>}
  </details>;
}
export default function StackContinuumResults({ dbId, job }: { dbId: string; job: StackColorJob }) {
  if (!job.continuum?.length) return null;
  return <section aria-label="Continuum results"><h4>Continuum results</h4>
    {job.continuum.map((line) => <Line key={`${job.job_id}:${line.role}`} dbId={dbId} jobId={job.job_id} line={line} />)}
  </section>;
}
