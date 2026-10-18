import type { SatelliteAnalysisStatus } from '../api/types';
import DetailSection from './DetailSection';

interface SatellitePanelProps {
  status?: SatelliteAnalysisStatus;
  isLoading: boolean;
  error?: string;
  predictError?: string;
  isPredicting: boolean;
  canPredict: boolean;
  overlayVisible: boolean;
  onToggleOverlay: () => void;
  onPredict: () => void;
  /** Folded, the heading says only how many tracks cross the frame. */
  open?: boolean;
  onToggleOpen?: () => void;
}

function plural(count: number, one: string): string {
  return `${count} ${one}${count === 1 ? '' : 's'}`;
}

export default function SatellitePanel({
  status,
  isLoading,
  error,
  predictError,
  isPredicting,
  canPredict,
  overlayVisible,
  onToggleOverlay,
  onPredict,
  open = true,
  onToggleOpen,
}: SatellitePanelProps) {
  const analysis = status?.analysis;
  const risk = analysis?.risk;
  const providerLabel = analysis?.catalog.provider === 'celes_trak_active'
    ? 'CelesTrak active'
    : analysis?.catalog.provider === 'seiza_mirror'
      ? 'Seiza mirror'
      : analysis?.catalog.provider === 'iau_sat_checker'
        ? 'IAU SatChecker'
        : undefined;

  const summary = isPredicting
    ? 'predicting…'
    : risk
      ? [
          plural(risk.track_count, 'crossing'),
          risk.high_risk_count ? `${risk.high_risk_count} bright` : null,
        ].filter(Boolean).join(' · ')
      : undefined;

  return (
    <DetailSection
      title="Satellite tracks"
      className="astrometry-section"
      headingClassName="astrometry-heading-row"
      data-testid="satellite-panel"
      badge={
        <span className={`astrometry-badge ${risk?.pixel_aligned_high_risk_count ? 'satellite-badge-high' : ''}`}>
          {isLoading
            ? 'Checking cache…'
            : risk?.pixel_aligned_count
              ? 'Pixel aligned'
              : analysis
                ? 'Predicted'
                : 'On demand'}
        </span>
      }
      summary={summary}
      open={open}
      onToggle={onToggleOpen}
    >
      {analysis ? (
        <button
          type="button"
          className="astrometry-toggle"
          aria-pressed={overlayVisible}
          onClick={onToggleOverlay}
        >
          <span className="astrometry-toggle-icon" aria-hidden="true">↗</span>
          {overlayVisible ? 'Track identifiers on' : 'Show track identifiers'}
          <span className="astrometry-toggle-count">{analysis.tracks.length}</span>
        </button>
      ) : (
        <button
          type="button"
          className="astrometry-solve"
          disabled={!canPredict || isLoading || isPredicting}
          title={canPredict
            ? undefined
            : 'This account can view cached tracks but cannot start a prediction.'}
          onClick={onPredict}
        >
          <span className="astrometry-toggle-icon" aria-hidden="true">↗</span>
          {isPredicting ? 'Predicting exposure tracks…' : 'Identify satellite tracks'}
        </button>
      )}

      {analysis && risk && (
        <dl className="astrometry-facts">
          <dt>Crossings</dt>
          <dd>{risk.track_count}</dd>
          <dt>Bright risk</dt>
          <dd className={risk.high_risk_count ? 'satellite-risk-high' : ''}>
            {risk.high_risk_count
              ? `${risk.high_risk_count} high`
              : risk.potentially_bright_count
                ? `${risk.potentially_bright_count} possible`
                : 'None predicted'}
          </dd>
          <dt>Pixel matches</dt>
          <dd>{risk.pixel_alignment_attempted ? risk.pixel_aligned_count : 'Unavailable'}</dd>
          <dt>Exposure</dt>
          <dd>{analysis.exposure.duration_seconds.toFixed(1)}s</dd>
          {providerLabel && (
            <>
              <dt>Element source</dt>
              <dd>{providerLabel}</dd>
            </>
          )}
          {analysis.catalog.query_epoch && (
            <>
              <dt>Elements for</dt>
              <dd>{new Date(analysis.catalog.query_epoch).toLocaleString()}</dd>
            </>
          )}
        </dl>
      )}

      {analysis?.tracks.slice(0, 6).map((track) => (
        <div className="satellite-track-fact" key={`${track.norad_id ?? track.label}`}>
          {track.norad_id ? (
            <a
              className="satellite-info-link"
              href={`https://www.n2yo.com/satellite/?s=${encodeURIComponent(track.norad_id)}`}
              target="_blank"
              rel="noopener noreferrer"
              title={`View NORAD ${track.norad_id} satellite information`}
            >
              {track.label}
              <span aria-hidden="true"> ↗</span>
            </a>
          ) : (
            <span>{track.label}</span>
          )}
          <span className={`satellite-risk-${track.risk_level}`}>
            {track.pixel_alignment?.status === 'detected'
              ? 'pixel match · '
              : track.pixel_alignment?.status === 'not_evaluated'
                ? 'pixel check unavailable · '
                : 'prediction only · '}
            {track.risk_level} · {Math.round(track.bright_trail_risk * 100)}%
          </span>
        </div>
      ))}

      {analysis && (
        <p className="astrometry-message">
          {risk?.pixel_aligned_count
            ? `Detected trails line up with ${risk.pixel_aligned_count} predicted satellite path${risk.pixel_aligned_count === 1 ? '' : 's'}. The satellite names are possible identifications.`
            : risk?.pixel_alignment_attempted
              ? 'These paths are predictions. The trail check found no match along them.'
              : `These paths are predictions from ${analysis.catalog.state.replaceAll('_', ' ')} orbital elements. The image could not be checked for matching trails.`}
        </p>
      )}
      {(error || predictError || analysis?.pixel_alignment_error || analysis?.catalog.warning) && (
        <p className="astrometry-message astrometry-message-warning">
          {predictError || error || analysis?.pixel_alignment_error || analysis?.catalog.warning}
        </p>
      )}
    </DetailSection>
  );
}
