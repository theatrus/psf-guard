import { afterAll, beforeAll, describe, it, expect } from 'vitest';
import { configure, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryRouter, RouterProvider, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import SequenceView from '../SequenceView';
import multiSessionFixture from '../../__fixtures__/sequence-analysis-multi-session.json';

/**
 * PR #872 review: a navigate() built from the rendered search, in the same
 * commit as an updateParams write, still drops that write. Here the entry
 * selection is normalised (frame 99999 is in no session) by updateParams,
 * then SequenceView's "all sessions" cursor effect (SequenceView.tsx:503)
 * replaces the URL from the rendered searchParams, putting 99999 back.
 */
const [first, second] = multiSessionFixture.data.sequences;
const rollup = {
  target_id: first.target_id,
  target_name: first.target_name,
  filter_name: first.filter_name,
  session_start: first.session_start,
  session_end: second.session_end,
  image_count: first.images.length + second.images.length,
  unavailable_image_count: 0,
  images: [...first.images, ...second.images].map(image => ({
    image_id: image.image_id,
    quality_score: image.quality_score,
    normalized_metrics: image.normalized_metrics,
    details: image.details,
  })),
  summary: multiSessionFixture.data.sequences[0].summary,
};

describe('SequenceView URL writes in one commit (PR #872 review)', () => {
  // SequenceView waits on three requests and the effects after them; a
  // busy full run can take longer than the default second.
  beforeAll(() => configure({ asyncUtilTimeout: 10_000 }));
  afterAll(() => configure({ asyncUtilTimeout: 1_000 }));

  it('keeps the normalised entry selection when the all-sessions cursor is written', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => HttpResponse.json({
        ...multiSessionFixture,
        data: { ...multiSessionFixture.data, target_filter_rollups: [rollup] },
      })),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => HttpResponse.json({
        success: true,
        data: [{ id: 2, name: 'NGC7000', ra: 314, dec: 44, active: true, image_count: 10, accepted_count: 0, rejected_count: 0, has_files: true }],
        error: null,
        status: 'ready',
      })),
      http.get('/api/db/:dbId/images', () => HttpResponse.json({ success: true, data: [], error: null, status: 'ready' })),
    );
    let search = '';
    function Probe() {
      search = useLocation().search;
      return null;
    }
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const router = createMemoryRouter(
      [{ path: '/sequence', element: <><SequenceView /><Probe /></> }],
      { initialEntries: [`/sequence?db=test&project=1&target=2&selected=${first.images[0].image_id},99999`] },
    );
    render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>);

    await screen.findByRole('button', { name: /All sessions/ });
    await waitFor(() => expect(new URLSearchParams(search).get('scoreScope')).toMatch(/^target-filter:/));
    // The normalised selection drops the frame no session holds.
    await waitFor(() => expect(new URLSearchParams(search).get('selected')).toBe(String(first.images[0].image_id)));
  });
});
