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
        const first = track.clipped_segments[0]?.[0];
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
                strokeWidth={track.risk_level === 'high' ? 3 : 2}
                strokeDasharray={track.risk_level === 'low' ? '6 5' : undefined}
                vectorEffect="non-scaling-stroke"
              />
            ))}
            {first && (
              <text
                x={first[0] + 7}
                y={first[1] - 7}
                fill={color}
                stroke="rgba(0,0,0,0.9)"
                strokeWidth={3}
                paintOrder="stroke"
                fontSize={14}
                fontWeight={700}
                vectorEffect="non-scaling-stroke"
              >
                {track.label}
              </text>
            )}
          </g>
        );
      })}
    </svg>
  );
}
