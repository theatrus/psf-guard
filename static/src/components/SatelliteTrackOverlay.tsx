import type { CSSProperties } from 'react';
import type { SatelliteAnalysis, SatelliteTrackPrediction } from '../api/types';

interface SatelliteTrackOverlayProps {
  analysis: SatelliteAnalysis;
  imageWidth: number;
  imageHeight: number;
  className?: string;
  style?: CSSProperties;
}

function trackColor(track: SatelliteTrackPrediction): string {
  if (track.risk_level === 'high') return '#ff4d5a';
  if (track.risk_level === 'possible') return '#ffd166';
  return '#43d9e6';
}

export default function SatelliteTrackOverlay({
  analysis,
  imageWidth,
  imageHeight,
  className,
  style,
}: SatelliteTrackOverlayProps) {
  return (
    <svg
      className={className}
      style={style}
      viewBox={`0 0 ${imageWidth} ${imageHeight}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={`${analysis.tracks.length} predicted satellite track${analysis.tracks.length === 1 ? '' : 's'}`}
    >
      {analysis.tracks.map((track, trackIndex) => {
        const aligned = track.pixel_alignment?.status === 'detected'
          ? track.pixel_alignment.aligned_segment
          : undefined;
        const first = aligned?.[0] ?? track.clipped_segments[0]?.[0];
        const color = trackColor(track);
        return (
          <g key={`${track.norad_id ?? track.label}:${trackIndex}`}>
            {track.clipped_segments.map((segment, segmentIndex) => (
              <line
                key={segmentIndex}
                x1={segment[0][0]}
                y1={segment[0][1]}
                x2={segment[1][0]}
                y2={segment[1][1]}
                stroke={color}
                strokeWidth={track.risk_level === 'high' ? 2.5 : 2}
                strokeDasharray={aligned || track.risk_level === 'low' ? '8 6' : undefined}
                opacity={aligned ? 0.72 : 1}
                vectorEffect="non-scaling-stroke"
              />
            ))}
            {aligned && (
              <line
                data-testid="pixel-aligned-satellite-track"
                x1={aligned[0][0]}
                y1={aligned[0][1]}
                x2={aligned[1][0]}
                y2={aligned[1][1]}
                stroke="#7cff6b"
                strokeWidth={4}
                vectorEffect="non-scaling-stroke"
              />
            )}
            {first && (
              <text
                x={first[0] + 7}
                y={first[1] - 7}
                fill={aligned ? '#7cff6b' : color}
                stroke="rgba(0,0,0,0.9)"
                strokeWidth={3}
                paintOrder="stroke"
                fontSize={14}
                fontWeight={700}
                vectorEffect="non-scaling-stroke"
              >
                {track.label}{aligned ? ' · pixel match' : ''}
              </text>
            )}
          </g>
        );
      })}
    </svg>
  );
}
