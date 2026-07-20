import type { CSSProperties } from 'react';
import { satelliteTrackOverlayObject } from '@seiza/astro-overlay';
import { AstroOverlay } from '@seiza/astro-overlay/react';
import type { SatelliteAnalysis } from '../api/types';

interface SatelliteTrackOverlayProps {
  analysis: SatelliteAnalysis;
  imageWidth: number;
  imageHeight: number;
  className?: string;
  style?: CSSProperties;
}

export default function SatelliteTrackOverlay({
  analysis,
  imageWidth,
  imageHeight,
  className,
  style,
}: SatelliteTrackOverlayProps) {
  const objects = analysis.tracks.map((track) => satelliteTrackOverlayObject({
    label: track.label,
    noradId: track.norad_id,
    cosparId: track.cospar_id,
    source: analysis.catalog?.source,
    catalogSource: analysis.catalog?.source,
    riskLevel: track.risk_level,
    maximumApparentRateArcsecPerSecond: track.maximum_apparent_rate_arcsec_per_second,
    segments: track.clipped_segments.map(([start, end]) => ({ start, end })),
    pixelAlignment: track.pixel_alignment == null ? undefined : {
      status: track.pixel_alignment.status,
      segments: track.pixel_alignment.aligned_segments.map((segment) => ({
        start: [segment.start.x, segment.start.y],
        end: [segment.end.x, segment.end.y],
      })),
    },
  }));

  return <AstroOverlay
    className={className}
    style={style}
    role="img"
    solution={{ image_width: imageWidth, image_height: imageHeight, objects }}
    layers={{ grid: false }}
    showCenter={false}
    aria-label={`${analysis.tracks.length} predicted satellite track${analysis.tracks.length === 1 ? '' : 's'}`}
  />;
}
