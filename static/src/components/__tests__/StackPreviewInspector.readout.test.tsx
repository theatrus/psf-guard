import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { OverlaySolution } from '@seiza/astro-overlay';
import { apiClient } from '../../api/client';
import { setMeasureOption } from '../../hooks/useMeasureOptions';
import type { PixelRect, PixelWindow } from '../../utils/pixelWindow';
import StackPreviewInspector from '../StackPreviewInspector';

/** A 100x100 mono stack whose sample at (x, y) is y·1000 + x. */
function stackWindow(rect: PixelRect): PixelWindow {
  const width = Math.max(0, Math.min(100, rect.x + rect.width) - rect.x);
  const height = Math.max(0, Math.min(100, rect.y + rect.height) - rect.y);
  const bytes = new Uint8Array(width * height * 4);
  const view = new DataView(bytes.buffer);
  for (let row = 0; row < height; row += 1) {
    for (let column = 0; column < width; column += 1) {
      view.setFloat32((row * width + column) * 4, (rect.y + row) * 1000 + rect.x + column, true);
    }
  }
  return {
    image_width: 100,
    image_height: 100,
    planes: 1,
    ...rect,
    width,
    height,
    values: btoa(String.fromCharCode(...bytes)),
  };
}

// North up, east left, 1"/px, centred on RA 10°, Dec +20°.
const solution: OverlaySolution = {
  image_width: 100,
  image_height: 100,
  wcs: { crval: [10, 20], crpix: [49.5, 49.5], cd: [[-1 / 3600, 0], [0, 1 / 3600]] },
};

function renderInspector(props: Partial<Parameters<typeof StackPreviewInspector>[0]> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <StackPreviewInspector
        eyebrow="Stack"
        title="M42"
        label="Ha"
        summary={[]}
        imageUrl="/stack.png"
        fitsUrl="/api/db/db-test/stack-previews/job/0/fits?v=rev"
        imageAlt="Stack preview"
        downloadLabel="Download FITS"
        solution={solution}
        onClose={() => undefined}
        {...props}
      />
    </QueryClientProvider>
  );
  const image = screen.getByTestId('stack-inspector-image') as HTMLImageElement;
  const canvas = image.parentElement as HTMLDivElement;
  Object.defineProperties(image, {
    naturalWidth: { configurable: true, value: 100 },
    naturalHeight: { configurable: true, value: 100 },
  });
  // 120px square: the 100px stack fits at 100%, 10px in from each edge.
  Object.defineProperty(canvas, 'getBoundingClientRect', {
    configurable: true,
    value: () => ({
      x: 0, y: 0, left: 0, top: 0, right: 120, bottom: 120, width: 120, height: 120,
      toJSON: () => ({}),
    }),
  });
  fireEvent.load(image);
  return canvas;
}

describe('StackPreviewInspector pointer readout', () => {
  beforeEach(() => {
    vi.spyOn(apiClient, 'getPixelWindow').mockImplementation(async (_path, rect) => stackWindow(rect));
  });

  afterEach(() => {
    setMeasureOption('value', false);
    setMeasureOption('sky', false);
    vi.restoreAllMocks();
  });

  it('shows nothing under the pointer until a readout is turned on', () => {
    const canvas = renderInspector();
    fireEvent.mouseMove(canvas, { clientX: 30, clientY: 22 });
    expect(screen.queryByTestId('pointer-readout')).not.toBeInTheDocument();
    expect(apiClient.getPixelWindow).not.toHaveBeenCalled();
  });

  it('reads the linear value under the pointer from the stack file', async () => {
    const canvas = renderInspector();
    fireEvent.click(screen.getByRole('button', { name: 'Value' }));
    fireEvent.mouseMove(canvas, { clientX: 30, clientY: 22 });

    expect(await screen.findByText('12020')).toBeInTheDocument();
    expect(screen.getByText('x 20 · y 12')).toBeInTheDocument();
    // The window route beside the FITS download, with its revision kept.
    expect(apiClient.getPixelWindow).toHaveBeenCalledWith(
      '/db/db-test/stack-previews/job/0/fits/window?v=rev',
      { x: 0, y: 0, width: 64, height: 64 },
    );

    fireEvent.mouseLeave(canvas);
    expect(screen.queryByTestId('pointer-readout')).not.toBeInTheDocument();
  });

  it('shows the sky position under the pointer from the stack’s solution', async () => {
    const canvas = renderInspector();
    fireEvent.click(screen.getByRole('button', { name: 'RA/Dec' }));
    fireEvent.mouseMove(canvas, { clientX: 30, clientY: 22 });

    const sky = await screen.findByTestId('pointer-readout-sky');
    // 29.5" east and 37.5" south of the centre.
    expect(sky.textContent).toMatch(/^RA 00h40m0[12]\.\ds Dec \+19°59′2[23]\.\d″$/);
    expect(screen.queryByTestId('pointer-readout-value')).not.toBeInTheDocument();
  });

  it('explains a stack without a solution and keeps RA/Dec off', () => {
    renderInspector({ solution: null });
    expect(screen.getByRole('button', { name: 'RA/Dec' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'RA/Dec' })).toHaveAttribute(
      'title',
      'No integrated frame of this stack is plate-solved yet',
    );
  });

  it('follows the pointer, on the side that keeps it in the viewer', () => {
    const canvas = renderInspector();
    fireEvent.click(screen.getByRole('button', { name: 'RA/Dec' }));

    fireEvent.mouseMove(canvas, { clientX: 30, clientY: 24 });
    const readout = screen.getByTestId('pointer-readout');
    expect(readout.style.left).toBe('25%');
    expect(readout.style.top).toBe('20%');
    expect(readout.style.transform).toBe('translate(14px, 14px)');

    // Near the bottom right, the box goes above and to the left.
    fireEvent.mouseMove(canvas, { clientX: 108, clientY: 102 });
    expect(readout.style.left).toBe('90%');
    expect(readout.style.top).toBe('85%');
    expect(readout.style.transform).toBe('translate(calc(-100% - 14px), calc(-100% - 14px))');
  });
});
