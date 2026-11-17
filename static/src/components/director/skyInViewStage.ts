import type { LonLat } from '../../utils/skyProjection';
import { DEFAULT_STAGE, deprojectOn, projectOn, skyAtStage, toStage, type Stage, type StageView } from './framingModel';

/** How far past the stage a line is still drawn, in stage units, so its
 *  end never shows inside. */
const MARGIN = 40;
/** A side mark every this many stage units along a line, this long. */
const MARK_EVERY = 46;
const MARK_LENGTH = 7;
/** At its widest the stage reaches 79° from its middle. A stretch of line
 *  with both ends further than this from the middle cannot cross it; drawn
 *  straight between where the stage would place its ends, far off on
 *  either side, it would seem to. */
const REACH_DEGREES = 85;

/** The sky under a point of the framing stage, as `[ra, dec]` in degrees:
 *  what the cover's raster asks for each of its pixels. */
export function stagePlace(view: StageView, viewFov: number, stage: Stage = DEFAULT_STAGE): (x: number, y: number) => readonly [number, number] {
  return (x, y) => {
    const at = skyAtStage(view, viewFov, x, y, stage);
    return [at.ra_degrees, at.dec_degrees];
  };
}

/** The part of the stretch from `a` to `b` inside the box from `low` to
 *  `high`, or null when none of it is. */
function clip(a: [number, number], b: [number, number], low: [number, number], high: [number, number]): [[number, number], [number, number]] | null {
  let [from, to] = [0, 1];
  for (const axis of [0, 1] as const) {
    const step = b[axis] - a[axis];
    for (const [edge, sign] of [[low[axis], 1], [high[axis], -1]] as const) {
      // Inside this edge where sign * (point - edge) >= 0.
      const start = sign * (a[axis] - edge);
      const change = sign * step;
      if (change === 0) {
        if (start < 0) return null;
      } else if (change > 0) from = Math.max(from, -start / change);
      else to = Math.min(to, -start / change);
    }
  }
  if (from > to) return null;
  const at = (t: number): [number, number] => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
  return [at(from), at(to)];
}

/**
 * Lines round the sky in view, drawn on the framing stage: the path of the
 * lines, and short marks along them that point into the sky in view, so
 * the two sides can be told apart without colour.
 *
 * A line runs with the sky in view on its left, seen with north up and
 * right ascension rising to the right. The stage shows the sky as it is
 * seen, east to the left, so there that side is on the right of the way
 * the line runs, however the sky is turned.
 *
 * Each stretch between two points of a line is cut to the stage and a
 * little past it. A stretch on the far side of the sky, with both ends
 * out of the stage's reach, is left out: nothing is drawn from beyond the
 * view.
 */
export function stageLines(lines: LonLat[][], view: StageView, viewFov: number, stage: Stage = DEFAULT_STAGE): { line: string; marks: string } {
  const low: [number, number] = [-MARGIN, -MARGIN];
  const high: [number, number] = [stage.width + MARGIN, stage.height + MARGIN];
  let path = '';
  let marks = '';
  const fixed = (value: number) => value.toFixed(1);
  const rad = Math.PI / 180;
  const middle = deprojectOn(view, [0, 0]);
  const [sin0, cos0] = [Math.sin(middle.dec_degrees * rad), Math.cos(middle.dec_degrees * rad)];
  const within = Math.cos(REACH_DEGREES * rad);
  for (const line of lines) {
    let last: [number, number] | null = null;
    let lastNear = false;
    let drawnTo: [number, number] | null = null;
    // How far along the line since the last mark, so marks keep their
    // spacing across the points of the line.
    let since = MARK_EVERY / 2;
    for (const [ra, dec] of line) {
      const offset = projectOn(view, { ra_degrees: ra, dec_degrees: dec });
      const here: [number, number] | null = offset && Number.isFinite(offset[0]) && Number.isFinite(offset[1]) ? toStage(offset, viewFov, stage) : null;
      const near = sin0 * Math.sin(dec * rad) + cos0 * Math.cos(dec * rad) * Math.cos((ra - middle.ra_degrees) * rad) > within;
      const [from, fromNear] = [last, lastNear];
      [last, lastNear] = [here, near];
      if (!from || !here || (!near && !fromNear)) continue;
      const shown = clip(from, here, low, high);
      if (!shown) continue;
      const [a, b] = shown;
      if (!drawnTo || Math.abs(drawnTo[0] - a[0]) > 0.05 || Math.abs(drawnTo[1] - a[1]) > 0.05) path += `M${fixed(a[0])} ${fixed(a[1])}`;
      path += `L${fixed(b[0])} ${fixed(b[1])}`;
      drawnTo = b;
      const length = Math.hypot(b[0] - a[0], b[1] - a[1]);
      if (length === 0) continue;
      const [u, v] = [(b[0] - a[0]) / length, (b[1] - a[1]) / length];
      for (let along = MARK_EVERY - since; along <= length; along += MARK_EVERY) {
        const [x, y] = [a[0] + u * along, a[1] + v * along];
        marks += `M${fixed(x)} ${fixed(y)}l${fixed(-v * MARK_LENGTH)} ${fixed(u * MARK_LENGTH)}`;
      }
      since = (since + length) % MARK_EVERY;
    }
  }
  return { line: path, marks };
}

/** The cover's raster for a stage: about `across` pixels wide, in the
 *  stage's shape. */
export function coverSize(stage: Stage, across: number): [number, number] {
  return [across, Math.max(1, Math.round((across * stage.height) / stage.width))];
}
