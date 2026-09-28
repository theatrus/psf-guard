import { useMemo } from 'react';
import type { DirectorFramingSummary } from '../../api/directorTypes';
import { STAGE_HEIGHT, STAGE_WIDTH, THUMB_HEIGHT, THUMB_WIDTH, framingGeometry, polygonPoints, stageCorners, thumbnailFov } from './framingModel';
import { useSurveyCutout } from './useSurveyCutout';

/** The plan's framing at a glance: the survey it was framed on, with its
 *  panels drawn where the plan puts them. Both come from the same server
 *  caches the framing view uses, so a list of plans costs little. */
export default function PlanThumbnail({ framing, name }: { framing: DirectorFramingSummary; name: string }) {
  const fov = thumbnailFov(framing);
  const request = useMemo(() => ({
    survey: framing.survey_id, ra: Number(framing.center.ra_degrees.toFixed(5)), dec: Number(framing.center.dec_degrees.toFixed(5)),
    fov: Number(fov.toFixed(5)), width: THUMB_WIDTH, height: THUMB_HEIGHT, rotation: 0,
  }), [framing.survey_id, framing.center.ra_degrees, framing.center.dec_degrees, fov]);
  const cutout = useSurveyCutout(request, 0);
  const geometry = useMemo(() => framing.panel ? framingGeometry({
    center: framing.center, position_angle_degrees: framing.position_angle_degrees, panel: framing.panel, mosaic: framing.mosaic,
    overlays: [], view: { center: framing.center, rotation_degrees: 0 },
  }) : null, [framing.center, framing.position_angle_degrees, framing.panel, framing.mosaic]);
  // The stage model works in 1024 × 768 units; the thumbnail is that view scaled down.
  const k = THUMB_WIDTH / STAGE_WIDTH;
  return <div className={`plan-thumb${cutout.status === 'failed' ? ' is-failed' : ''}`} role="img" aria-label={`Framing of ${name} on ${framing.survey_id.replace(/_/g, ' ')}`} title={cutout.status === 'failed' ? cutout.error : undefined}>
    {cutout.image ? <img src={cutout.image.url} alt="" draggable={false} /> : <div className="plan-thumb-empty">{cutout.status === 'failed' ? 'No survey image' : 'Loading sky...'}</div>}
    <svg viewBox={`0 0 ${STAGE_WIDTH * k} ${STAGE_HEIGHT * k}`} aria-hidden="true">
      <g transform={`scale(${k})`}>
        {geometry?.panels.map(panel => { const corners = stageCorners(panel.corners, { anchor: framing.center, offset: [0, 0] }); return corners && <polygon key={panel.id} className="plan-thumb-panel" points={polygonPoints(corners, fov)} />; })}
      </g>
    </svg>
  </div>;
}
