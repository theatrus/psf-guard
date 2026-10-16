import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import DustOverlay from '../DustOverlay';
import { featureMark, vignettingMark } from '../dustLabels';

/** The highest point a drawn shape reaches: a circle's, a rect's, or a
 *  path's of absolute moves and lines with relative `v` steps. */
function topOf(shape: Element): number {
  if (shape.tagName === 'circle') return Number(shape.getAttribute('cy')) - Number(shape.getAttribute('r'));
  if (shape.tagName === 'rect') return Number(shape.getAttribute('y'));
  const tokens = (shape.getAttribute('d') ?? '').trim().split(/\s+/);
  let y = 0;
  let top = Infinity;
  for (let i = 0; i < tokens.length; i += 1) {
    const command = tokens[i];
    if (command === 'M' || command === 'L') { y = Number(tokens[i + 2]); i += 2; }
    else if (command === 'A') { y = Number(tokens[i + 7]); i += 7; }
    else if (command === 'v') { y += Number(tokens[i + 1]); i += 1; }
    else if (command === 'h') { i += 1; continue; }
    else continue;
    top = Math.min(top, y);
  }
  return top;
}

describe('DustOverlay geometry', () => {
  it('keeps a bottom-edge vignetting band on the frame and near its edge', () => {
    // A band 50 px tall along the whole bottom edge of a 6000 x 4000 frame.
    const band = vignettingMark({ edge: 'bottom', x: 3000, y: 3975, area: 6000 * 50, depth: 0.02 }, 'v');
    const { container } = render(
      <DustOverlay width={6000} height={4000} marks={[]} vignetting={[band]} pixelScale={0.2}
        picked={null} onPick={() => {}} className="t" />,
    );
    const shape = container.querySelector('[data-shape="vignetting"] circle, [data-shape="vignetting"] rect, [data-shape="vignetting"] path')!;
    // The shadow reaches no higher than the band does.
    expect(topOf(shape)).toBeGreaterThanOrEqual(3900);
  });

  it('keeps a disk label near the right edge on the frame', () => {
    const disk = featureMark({ x: 5800, y: 2000, left: 5500, top: 1700, right: 6000, bottom: 2300, pixels: 270000,
      depth: 0.003, shape: 'disk', radius: 300 }, 'd');
    const { container } = render(
      <DustOverlay width={6000} height={4000} marks={[disk]} vignetting={[]} pixelScale={0.2}
        picked={null} onPick={() => {}} className="t" />,
    );
    const text = container.querySelector('text')!;
    const anchor = text.getAttribute('text-anchor') ?? 'start';
    const x = Number(text.getAttribute('x'));
    // Drawn from its x toward the right, it would leave the 6000-px frame.
    expect(anchor === 'end' || x < 5600).toBe(true);
  });

  it('dims session dust this flat does not show, and draws a corner shadow in its corner', () => {
    const disk = featureMark({ x: 2000, y: 2000, left: 1700, top: 1700, right: 2300, bottom: 2300, pixels: 270000,
      depth: 0.003, shape: 'disk', radius: 300 }, 'd', { unseen: true });
    const corner = vignettingMark({ edge: 'bottom_left', x: 300, y: 3700, area: 200_000, depth: 0.02 }, 'v');
    const { container } = render(
      <DustOverlay width={6000} height={4000} marks={[disk]} vignetting={[corner]} pixelScale={0.2}
        picked={null} onPick={() => {}} className="t" />,
    );
    expect(container.querySelector('[data-shape="disk"]')).toHaveClass('is-unseen');
    // A quarter disc from the bottom-left corner.
    const d = container.querySelector('[data-shape="vignetting"] path')!.getAttribute('d')!;
    expect(d.startsWith('M 0 4000')).toBe(true);
    expect(topOf(container.querySelector('[data-shape="vignetting"] path')!)).toBeGreaterThan(3400);
  });
});
