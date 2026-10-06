import { useMemo } from 'react';
import type { MosaicPanel, ProjectMosaic, SkyPreview } from '../../api/types';
import type { DirectorSkyPosition } from '../../api/directorTypes';
import { clampFov, DEFAULT_STAGE, projectOn, stackMatrix, toStage, viewAt } from '../director/framingModel';
import { tanPixelToSky } from '../../utils/skyProjection';
import { mosaicLabel } from '../../hooks/useProjectMosaic';

type Solved = MosaicPanel & { preview: SkyPreview & { wcs: NonNullable<SkyPreview['wcs']> } };

const solved = (panel: MosaicPanel): panel is Solved => !!panel.preview?.wcs && panel.preview.width > 0 && panel.preview.height > 0;

function skyAt(preview: Solved['preview'], x: number, y: number): DirectorSkyPosition {
  const [ra, dec] = tanPixelToSky(preview.wcs, x, y);
  return { ra_degrees: ra, dec_degrees: dec };
}

/** The corners of a solved preview on the sky, in drawing order. */
function corners(preview: Solved['preview']): DirectorSkyPosition[] {
  const { width: w, height: h } = preview;
  return [[0, 0], [w, 0], [w, h], [0, h]].map(([x, y]) => skyAt(preview, x, y));
}

/** Mean of unit vectors: the middle of a few nearby sky positions. */
function middle(positions: DirectorSkyPosition[]): DirectorSkyPosition {
  const rad = Math.PI / 180;
  let [x, y, z] = [0, 0, 0];
  for (const p of positions) {
    const dec = p.dec_degrees * rad;
    const ra = p.ra_degrees * rad;
    x += Math.cos(dec) * Math.cos(ra);
    y += Math.cos(dec) * Math.sin(ra);
    z += Math.sin(dec);
  }
  const ra = Math.atan2(y, x) / rad;
  return { ra_degrees: (ra + 360) % 360, dec_degrees: Math.atan2(z, Math.hypot(x, y)) / rad };
}

/** Each panel's latest stack preview, laid out as the mosaic's grid, and a
 *  sky overview that places every solved preview by its plate solve. For
 *  looking only: nothing is stitched, and each panel stacks on its own. */
export default function MosaicStacks({ mosaic }: { mosaic: ProjectMosaic }) {
  const sky = useMemo(() => {
    const placed = mosaic.panels.filter(solved);
    if (placed.length === 0) return null;
    const stage = DEFAULT_STAGE;
    const allCorners = placed.flatMap(panel => corners(panel.preview));
    const center = middle(allCorners);
    const probe = viewAt(center, center);
    let [halfX, halfY] = [0, 0];
    for (const corner of allCorners) {
      const offset = projectOn(probe, corner);
      if (!offset) continue;
      halfX = Math.max(halfX, Math.abs(offset[0]));
      halfY = Math.max(halfY, Math.abs(offset[1]));
    }
    const fov = clampFov(Math.max(2 * halfX, (2 * halfY * stage.width) / stage.height) * 1.15);
    const panels = placed.flatMap(panel => {
      const matrix = stackMatrix(panel.preview, probe, fov, stage);
      const at = projectOn(probe, skyAt(panel.preview, panel.preview.width / 2, panel.preview.height / 2));
      const outline = corners(panel.preview).map(corner => projectOn(probe, corner));
      if (!matrix || !at || outline.some(point => point === null)) return [];
      return [{
        panel,
        matrix,
        label: toStage(at, fov, stage),
        outline: outline.map(point => toStage(point!, fov, stage).map(v => v.toFixed(1)).join(',')).join(' '),
      }];
    });
    return { stage, panels };
  }, [mosaic]);

  const unsolved = mosaic.panels.filter(panel => panel.preview && !solved(panel));
  const missing = mosaic.panels.filter(panel => !panel.preview);

  return (
    <section className="mosaic-stacks" aria-label={`${mosaicLabel(mosaic)}: stacks`}>
      <h2>{mosaicLabel(mosaic)}</h2>
      <p className="muted">
        Each panel's latest stack. They are not stitched: each panel stacks on its own.
      </p>
      <div className="mosaic-stacks-body">
        <div
          className="mosaic-stacks-grid"
          style={{ gridTemplateColumns: `repeat(${mosaic.columns}, minmax(0, 1fr))` }}
        >
          {mosaic.panels.map(panel => (
            <figure
              key={panel.target_id}
              className="mosaic-stacks-cell"
              style={{ gridRow: panel.row, gridColumn: panel.column }}
            >
              {panel.preview
                ? <img src={panel.preview.url} alt={`${panel.panel_id} stack`} loading="lazy" />
                : <div className="mosaic-stacks-empty">No stack yet</div>}
              <figcaption title={panel.target_name}>
                <strong>{panel.panel_id}</strong> {panel.target_name}
                {panel.preview?.filter && <span className="muted"> · {panel.preview.filter}</span>}
              </figcaption>
            </figure>
          ))}
        </div>
        {sky && sky.panels.length > 0 && (
          <figure className="mosaic-stacks-sky">
            <svg
              viewBox={`0 0 ${sky.stage.width} ${sky.stage.height}`}
              role="img"
              aria-label="Panels placed on the sky by their plate solves"
            >
              <rect width={sky.stage.width} height={sky.stage.height} className="mosaic-stacks-sky-bg" />
              {sky.panels.map(({ panel, matrix }) => (
                <image
                  key={panel.target_id}
                  href={panel.preview.url}
                  width={panel.preview.width}
                  height={panel.preview.height}
                  transform={matrix}
                  preserveAspectRatio="none"
                />
              ))}
              {sky.panels.map(({ panel, outline, label }) => (
                <g key={panel.target_id} className="mosaic-stacks-sky-panel">
                  <polygon points={outline} />
                  <text x={label[0]} y={label[1]} textAnchor="middle" dominantBaseline="middle">{panel.panel_id}</text>
                </g>
              ))}
            </svg>
            <figcaption className="muted">
              On the sky by plate solve, north up.
              {unsolved.length > 0 && ` Not solved yet: ${unsolved.map(panel => panel.panel_id).join(', ')}.`}
              {missing.length > 0 && ` No stack yet: ${missing.map(panel => panel.panel_id).join(', ')}.`}
            </figcaption>
          </figure>
        )}
      </div>
    </section>
  );
}
