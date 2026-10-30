import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { OverlaySolution } from '@seiza/astro-overlay';
import { apiClient } from '../../api/client';
import type { AstrometryAnalysis } from '../../api/types';
import { setMeasureOption } from '../../hooks/useMeasureOptions';
import { setSkyOverlay } from '../../hooks/useSkyOverlay';
import type { PixelRect, PixelWindow } from '../../utils/pixelWindow';
import StackPreviewInspector from '../StackPreviewInspector';

const linearFits = '/api/db/db-test/stack-previews/job/0/fits?v=rev';

/** A 100x100 stack whose samples are all 7. */
function stackWindow(rect: PixelRect): PixelWindow {
  const width = Math.max(0, Math.min(100, rect.x + rect.width) - rect.x);
  const height = Math.max(0, Math.min(100, rect.y + rect.height) - rect.y);
  const bytes = new Uint8Array(width * height * 4);
  const view = new DataView(bytes.buffer);
  for (let index = 0; index < width * height; index += 1) view.setFloat32(index * 4, 7, true);
  return { image_width: 100, image_height: 100, planes: 1, ...rect, width, height, values: btoa(String.fromCharCode(...bytes)) };
}

// North up, east left, 1"/px, with the given RA and Dec at pixel (50, 50).
function solutionAt(ra: number, dec: number): OverlaySolution {
  return {
    image_width: 100,
    image_height: 100,
    center_ra_deg: ra,
    center_dec_deg: dec,
    pixel_scale_arcsec_per_pixel: 1,
    wcs: { crval: [ra, dec], crpix: [50, 50], cd: [[-1 / 3600, 0], [0, 1 / 3600]] },
    objects: [],
  } as OverlaySolution;
}

function analysis(solution?: OverlaySolution): AstrometryAnalysis {
  return {
    image_id: 0,
    status: solution ? 'solved' : 'catalog_only',
    mode: solution ? 'hinted' : undefined,
    solution,
    catalog_hits: [],
    source_fingerprint: { canonical_path: '/stacks/job/0.fits', size_bytes: 1, modified_unix_seconds: 1 },
    computed_at: 1,
  };
}

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
        fitsUrl={linearFits}
        imageAlt="Stack preview"
        downloads={[
          { label: 'Download linear FITS', href: linearFits },
          { label: 'Download deconvolved linear FITS', href: '/api/db/db-test/stack-previews/stretch/s/fits' },
        ]}
        analysisFitsUrl={linearFits}
        solution={solutionAt(10, 20)}
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
  Object.defineProperty(canvas, 'getBoundingClientRect', {
    configurable: true,
    value: () => ({ x: 0, y: 0, left: 0, top: 0, right: 120, bottom: 120, width: 120, height: 120, toJSON: () => ({}) }),
  });
  fireEvent.load(image);
  return { image, canvas };
}

describe('StackPreviewInspector tools', () => {
  beforeEach(() => {
    vi.spyOn(apiClient, 'getPixelWindow').mockImplementation(async (_path, rect) => stackWindow(rect));
    vi.spyOn(apiClient, 'getStackAstrometry').mockResolvedValue(analysis());
  });

  afterEach(() => {
    setMeasureOption('value', false);
    setMeasureOption('sky', false);
    setSkyOverlay(false);
    vi.restoreAllMocks();
  });

  it('lists every FITS of the stack', () => {
    renderInspector();
    expect(screen.getByRole('link', { name: 'Download linear FITS' })).toHaveAttribute('href', linearFits);
    expect(screen.getByRole('link', { name: 'Download deconvolved linear FITS' })).toHaveAttribute('download');
  });

  it('solves the stack itself, then reads RA/Dec and draws the sky from its own solution', async () => {
    const solve = vi.spyOn(apiClient, 'solveStackAstrometry').mockResolvedValue(analysis(solutionAt(30, 40)));
    const { canvas } = renderInspector();
    expect(apiClient.getStackAstrometry).toHaveBeenCalledWith(linearFits);

    // Before the solve, RA/Dec comes from the frames' solution.
    fireEvent.click(screen.getByRole('button', { name: 'RA/Dec' }));
    // The stack sits 10px in, at 100%: this is pixel (50, 50).
    fireEvent.mouseMove(canvas, { clientX: 60.5, clientY: 60.5 });
    expect(screen.getByTestId('pointer-readout-sky').textContent).toMatch(/^RA 00h40m00\.0s Dec \+20°00′00\.0″$/);

    fireEvent.click(await screen.findByRole('button', { name: /Solve field/ }));
    await waitFor(() => expect(solve).toHaveBeenCalledWith(linearFits));
    await waitFor(() => expect(screen.getByTestId('pointer-readout-sky').textContent)
      .toMatch(/^RA 02h00m00\.0s Dec \+40°00′00\.0″$/));

    fireEvent.click(screen.getByRole('button', { name: /Show sky overlay/ }));
    expect(screen.getByTestId('stack-inspector-astrometry-overlay')).toBeInTheDocument();
  });

  it('marks stars with S and shows the PSF view with P, as the image viewer does', () => {
    const { image } = renderInspector({ artifactSource: {
      kind: 'mono', dbId: 'db-test', jobId: 'a'.repeat(64), groupIndex: 0, artifactRevision: 'rev',
    }, artifactEnabled: true });

    act(() => { fireEvent.keyDown(document, { key: 's', code: 'KeyS' }); });
    expect(image.getAttribute('src')).toBe('/api/db/db-test/stack-previews/job/0/fits/annotated?v=rev&rv=2&size=large');
    expect(screen.getByRole('status')).toHaveTextContent('Marking stars…');
    expect(screen.getByRole('button', { name: 'Find source artifact' })).toBeDisabled();

    act(() => { fireEvent.keyDown(document, { key: 'p', code: 'KeyP' }); });
    expect(screen.getByTestId('stack-inspector-psf').getAttribute('src'))
      .toBe('/api/db/db-test/stack-previews/job/0/fits/psf?v=rev&num_stars=9&psf_type=moffat&sort_by=r2&selection=top-n');
    expect(image.getAttribute('src')).toBe('/stack.png');

    act(() => { fireEvent.keyDown(document, { key: 'p', code: 'KeyP' }); });
    expect(screen.queryByTestId('stack-inspector-psf')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Find source artifact' })).toBeEnabled();
  });

  it('offers no sky, stars or PSF for an image with no sky, such as a master', () => {
    renderInspector({ analysisFitsUrl: undefined, solution: undefined });
    expect(apiClient.getStackAstrometry).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: /Solve field/ })).not.toBeInTheDocument();
    act(() => { fireEvent.keyDown(document, { key: 's', code: 'KeyS' }); });
    expect((screen.getByTestId('stack-inspector-image') as HTMLImageElement).getAttribute('src')).toBe('/stack.png');
    expect(screen.queryByText(/S Stars/)).not.toBeInTheDocument();
  });
});
