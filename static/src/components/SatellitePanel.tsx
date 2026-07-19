import type { SatelliteAnalysisStatus } from '../api/types';

interface SatellitePanelProps {
  status?: SatelliteAnalysisStatus;
  isLoading: boolean;
  error?: string;
  predictError?: string;
  isPredicting: boolean;
  overlayVisible: boolean;
  onToggleOverlay: () => void;
  onPredict: () => void;
}

export default function SatellitePanel({
  status,
  isLoading,
  error,
  predictError,
  isPredicting,
  overlayVisible,
  onToggleOverlay,
  onPredict,
}: SatellitePanelProps) {
  const analysis = status?.analysis;
  const risk = analysis?.risk;

  return (
    <section className="info-section astrometry-section" data-testid="satellite-panel">
      <div className="astrometry-heading-row">
        <h3>Satellite tracks</h3>
        <span className={`astrometry-badge ${risk?.high_risk_count ? 'satellite-badge-high' : ''}`}>
          {isLoading ? 'Checking cache…' : analysis ? 'Predicted' : 'On demand'}
        </span>
      </div>

      {analysis ? (
        <button
          type="button"
          className={`astrometry-toggle ${overlayVisible ? 'active' : ''}`}
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
          disabled={isLoading || isPredicting}
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
          <dt>Exposure</dt>
          <dd>{analysis.exposure.duration_seconds.toFixed(1)}s</dd>
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
            {track.risk_level} · {Math.round(track.bright_trail_risk * 100)}%
          </span>
        </div>
      ))}

      {analysis && (
        <p className="astrometry-message">
          Orbital prediction from {analysis.catalog.state.replaceAll('_', ' ')} elements; it does not claim a trail was detected in the pixels.
        </p>
      )}
      {(error || predictError || analysis?.catalog.warning) && (
        <p className="astrometry-message astrometry-message-warning">
          {predictError || error || analysis?.catalog.warning}
        </p>
      )}
    </section>
  );
}
