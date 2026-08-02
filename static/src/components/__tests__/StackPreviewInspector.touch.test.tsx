import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import StackPreviewInspector from '../StackPreviewInspector';

describe('StackPreviewInspector touch selection', () => {
  it('turns a one-finger drag into an artifact search region', () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <StackPreviewInspector
          eyebrow="Stack"
          title="Full size"
          label="Ha"
          summary={[]}
          imageUrl="/stack.png"
          fitsUrl="/stack.fits"
          imageAlt="Stack preview"
          downloadLabel="Download FITS"
          artifactSource={{
            kind: 'mono',
            dbId: 'db-test',
            jobId: 'a'.repeat(64),
            groupIndex: 0,
            artifactRevision: 'revision',
          }}
          artifactEnabled
          onClose={() => undefined}
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
      value: () => ({
        x: 0,
        y: 0,
        left: 0,
        top: 0,
        right: 100,
        bottom: 100,
        width: 100,
        height: 100,
        toJSON: () => ({}),
      }),
    });
    fireEvent.load(image);
    fireEvent.click(screen.getByRole('button', { name: 'Find source artifact' }));
    fireEvent.touchStart(canvas, {
      touches: [{ clientX: 10, clientY: 12 }],
    });
    fireEvent.touchMove(canvas, {
      touches: [{ clientX: 35, clientY: 40 }],
    });
    fireEvent.touchEnd(canvas, { touches: [] });

    expect(screen.getByTestId('stack-artifact-region')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Search this region' })).toBeEnabled();
  });
});
