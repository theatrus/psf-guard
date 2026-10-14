import type { AstrometryAnalysis, CatalogHit } from '../api/types';
import { fieldRotation } from '../utils/fieldRotation';
import DetailSection from './DetailSection';

interface AstrometryPanelProps {
  analysis?: AstrometryAnalysis;
  isLoading: boolean;
  error?: string;
  solveError?: string;
  isSolving: boolean;
  canSolve: boolean;
  overlayVisible: boolean;
  onToggleOverlay: () => void;
  onSolve: () => void;
  /** Folded, the heading says only whether and where the field solved. */
  open?: boolean;
  onToggleOpen?: () => void;
}

function formatRa(raDeg: number): string {
  const totalHours = ((raDeg % 360) + 360) % 360 / 15;
  const hours = Math.floor(totalHours);
  const minutesFloat = (totalHours - hours) * 60;
  const minutes = Math.floor(minutesFloat);
  const seconds = (minutesFloat - minutes) * 60;
  return `${String(hours).padStart(2, '0')}h ${String(minutes).padStart(2, '0')}m ${seconds.toFixed(1).padStart(4, '0')}s`;
}

function formatDec(decDeg: number): string {
  const sign = decDeg < 0 ? '−' : '+';
  const absolute = Math.abs(decDeg);
  const degrees = Math.floor(absolute);
  const minutesFloat = (absolute - degrees) * 60;
  const minutes = Math.floor(minutesFloat);
  const seconds = (minutesFloat - minutes) * 60;
  return `${sign}${String(degrees).padStart(2, '0')}° ${String(minutes).padStart(2, '0')}′ ${seconds.toFixed(0).padStart(2, '0')}″`;
}

function objectSubtitle(hit: CatalogHit): string {
  const details = [hit.kind.replaceAll('-', ' ')];
  if (hit.mag != null) details.push(`mag ${hit.mag.toFixed(1)}`);
  if (hit.major_arcmin != null) {
    const size = hit.minor_arcmin != null
      ? `${hit.major_arcmin.toFixed(1)}′ × ${hit.minor_arcmin.toFixed(1)}′`
      : `${hit.major_arcmin.toFixed(1)}′`;
    details.push(size);
  }
  if (hit.extent_only) details.push('edge overlap');
  return details.join(' · ');
}

/** A folded panel's line: whether the pixels solved, where the target fell
 *  and how many objects the overlay marks. The badge beside it says how the
 *  field was found; a header WCS is not a solve. */
function skySummary(analysis: AstrometryAnalysis): string {
  const solution = analysis.solution;
  if (!solution) return analysis.status === 'failed' ? 'solve failed' : 'not solved';
  const parts = analysis.mode === 'embedded_wcs' ? [] : ['solved'];
  if (analysis.pointing) {
    parts.push(analysis.pointing.target_in_frame ? 'target in frame' : 'target outside frame');
  }
  const objects = solution.objects?.length ?? 0;
  parts.push(`${objects} object${objects === 1 ? '' : 's'}`);
  return parts.join(' · ');
}

export default function AstrometryPanel({
  analysis,
  isLoading,
  error,
  solveError,
  isSolving,
  canSolve,
  overlayVisible,
  onToggleOverlay,
  onSolve,
  open = true,
  onToggleOpen,
}: AstrometryPanelProps) {
  const section = {
    title: 'Sky context',
    className: 'astrometry-section',
    headingClassName: 'astrometry-heading-row',
    'data-testid': 'astrometry-panel',
    open,
    onToggle: onToggleOpen,
  };

  if (isLoading) {
    return (
      <DetailSection {...section} badge={<span className="astrometry-badge">Reading headers…</span>} />
    );
  }

  if (error || !analysis) {
    return (
      <DetailSection {...section} summary="unavailable">
        <p className="astrometry-message">{error || 'Astrometry information is unavailable.'}</p>
      </DetailSection>
    );
  }

  const solution = analysis.solution;
  const rotation = solution?.wcs ? fieldRotation(solution.wcs) : null;
  const badge = solution
    ? analysis.mode === 'hinted'
      ? 'Hinted solve'
      : analysis.mode === 'blind'
        ? 'Blind solve'
        : 'Embedded FITS WCS'
    : analysis.catalog_scope === 'nearby_target'
      ? 'Nearby catalog'
    : analysis.status === 'catalog_only'
      ? 'Expected field'
      : 'Unavailable';
  const coordinate = solution?.center_ra_deg != null && solution.center_dec_deg != null
    ? { ra: solution.center_ra_deg, dec: solution.center_dec_deg }
    : analysis.hint_source
      ? { ra: analysis.hint_source.ra_deg, dec: analysis.hint_source.dec_deg }
      : analysis.expected_source
        ? { ra: analysis.expected_source.ra_deg, dec: analysis.expected_source.dec_deg }
        : null;
  const shownHits = analysis.catalog_hits.slice(0, 6);

  return (
    <DetailSection
      {...section}
      badge={<span className={`astrometry-badge astrometry-badge-${analysis.status}`}>{badge}</span>}
      summary={isSolving ? 'solving…' : solveError ? 'solve failed' : skySummary(analysis)}
    >
      {solution && (
        <button
          type="button"
          className="astrometry-toggle"
          aria-pressed={overlayVisible}
          onClick={onToggleOverlay}
        >
          <span className="astrometry-toggle-icon" aria-hidden="true">◎</span>
          {overlayVisible ? 'Sky overlay on' : 'Show sky overlay'}
          <span className="astrometry-toggle-count">{solution.objects?.length ?? 0}</span>
        </button>
      )}

      {!solution && (
        <button
          type="button"
          className="astrometry-solve"
          disabled={!canSolve || isSolving}
          title={canSolve ? undefined : 'This account can view cached solutions but cannot start a plate solve.'}
          onClick={onSolve}
        >
          <span className="astrometry-toggle-icon" aria-hidden="true">◎</span>
          {isSolving
            ? 'Solving field…'
            : analysis.status === 'failed'
              ? 'Retry plate solve'
              : 'Solve field'}
        </button>
      )}

      {coordinate && (
        <dl className="astrometry-facts">
          <dt>Center</dt>
          <dd>{formatRa(coordinate.ra)} · {formatDec(coordinate.dec)}</dd>
          {solution?.pixel_scale_arcsec_per_pixel != null && (
            <>
              <dt>Scale</dt>
              <dd>{solution.pixel_scale_arcsec_per_pixel.toFixed(3)}″/px</dd>
            </>
          )}
          {rotation && (
            <>
              <dt>Rotation</dt>
              <dd title="Direction of celestial north from image up, positive toward image right">
                {rotation.degrees.toFixed(1)}°{rotation.mirrored ? ' · mirrored' : ''}
              </dd>
            </>
          )}
          {analysis.pointing && (
            <>
              <dt>Target</dt>
              <dd className={analysis.pointing.target_in_frame ? 'target-in-frame' : 'target-out-of-frame'}>
                {analysis.pointing.target_in_frame ? 'In frame' : 'Outside frame'} · {analysis.pointing.separation_arcsec.toFixed(0)}″ from center
              </dd>
            </>
          )}
          {analysis.catalog_scope === 'nearby_target' && analysis.catalog_radius_deg != null && (
            <>
              <dt>Search</dt>
              <dd>Within {analysis.catalog_radius_deg.toFixed(1)}° · field size unknown</dd>
            </>
          )}
        </dl>
      )}

      {shownHits.length > 0 && (
        <div className="astrometry-objects">
          <div className="astrometry-list-heading">
            <span>
              {analysis.catalog_scope === 'nearby_target'
                ? 'Objects near target'
                : 'Expected objects in field'}
            </span>
            <span>{analysis.catalog_hits.length}</span>
          </div>
          <ol>
            {shownHits.map((hit) => (
              <li key={hit.stable_id || `${hit.source}:${hit.name}`}>
                <div className="astrometry-object-title">
                  <span>{hit.name}</span>
                  {hit.common_name && <span>{hit.common_name}</span>}
                </div>
                <div className="astrometry-object-meta">{objectSubtitle(hit)}</div>
                <div className="astrometry-object-source">{hit.source}</div>
              </li>
            ))}
          </ol>
          {analysis.catalog_hits.length > shownHits.length && (
            <div className="astrometry-more">+{analysis.catalog_hits.length - shownHits.length} lower-prominence matches</div>
          )}
        </div>
      )}

      {(solveError || analysis.error) && (
        <p className="astrometry-message astrometry-message-warning">
          {solveError || analysis.error}
        </p>
      )}
    </DetailSection>
  );
}
