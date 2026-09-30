import { describe, expect, it } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import StackPreviewPanel from '../StackPreviewPanel';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });
const readyGroup = (index: number, targetId: number, targetName: string) => ({
  index, target_id: targetId, target_name: targetName, filter_name: 'Ha', state: 'ready', phase: 'ready',
  total_candidates: 2, eligible_frames: 2, quality_excluded: 0, missing_files: 0, processed_frames: 2,
  accepted_frames: 2, rejected_frames: 0, output_channels: 1, reference_image_id: index + 1, total_exposure_seconds: 600,
  preview_url: null, fits_url: null, error: null,
  calibration: { state: 'none', bias_frames: 0, dark_frames: 0, dark_flat_frames: 0, flat_frames: 0, warning: null },
  input_images: [], frames: [],
});

function mount(targetId: number | null) {
  // The project remembers a stack for each of two mosaic panels.
  server.use(
    http.get('/api/stack-activity', () => ok({ schema_version: 1, active: [] })),
    http.get('/api/db/test/projects/1/stack-previews/latest', () => ok({ groups: [
      { job_id: 'job', artifact_revision: 'rev', accepted_only: false, group: readyGroup(0, 27, 'Heart Panel 1') },
      { job_id: 'job', artifact_revision: 'rev', accepted_only: false, group: readyGroup(1, 28, 'Heart Panel 2') },
    ] })),
    http.get('/api/db/test/projects/1/stack-previews/color', () => ok({ targets: [], jobs: [] })),
    http.get('/api/db/test/stack-previews/job/:index/stretch', () => ok(null)),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  const images = targetId === 28
    ? [{ id: 3, target_id: 28, target_name: 'Heart Panel 2', filter_name: 'Ha', grading_status: 1 }]
    : [
        { id: 1, target_id: 27, target_name: 'Heart Panel 1', filter_name: 'Ha', grading_status: 1 },
        { id: 3, target_id: 28, target_name: 'Heart Panel 2', filter_name: 'Ha', grading_status: 1 },
      ];
  return render(<StackPreviewPanel dbId="test" projectId={1} images={images} selectionSource="visible" onOpenImage={() => {}} targetId={targetId} />, { wrapper: Wrapper });
}

describe('stack previews for a chosen target', () => {
  it("shows only the chosen panel's stacks, not the rest of the mosaic", async () => {
    mount(28);
    await waitFor(() => expect(screen.getAllByText('Heart Panel 2').length).toBeGreaterThan(0));
    expect(screen.queryByText('Heart Panel 1')).not.toBeInTheDocument();
  });

  it('shows every panel when no target is chosen', async () => {
    mount(null);
    await waitFor(() => expect(screen.getAllByText('Heart Panel 1').length).toBeGreaterThan(0));
    expect(screen.getAllByText('Heart Panel 2').length).toBeGreaterThan(0);
  });
});
