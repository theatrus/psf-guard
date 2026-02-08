import { describe, it, expect } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import SequenceView from '../SequenceView';
import normalFixture from '../../__fixtures__/sequence-analysis-normal.json';
import cloudsFixture from '../../__fixtures__/sequence-analysis-clouds.json';
import multiSessionFixture from '../../__fixtures__/sequence-analysis-multi-session.json';
import emptyFixture from '../../__fixtures__/sequence-analysis-empty.json';

// Mock images data that aligns with the normal fixture's image IDs
const mockImages = normalFixture.data.sequences[0].images.map((img, i) => ({
  id: img.image_id,
  project_id: 1,
  project_name: 'Test Project',
  project_display_name: 'Test Project',
  target_id: 1,
  target_name: 'M42',
  acquired_date: 1705352400 + i * 300,
  filter_name: 'L',
  grading_status: 0,
  reject_reason: null,
  metadata: { FileName: `image_${img.image_id}.fits` },
  filesystem_path: `/images/image_${img.image_id}.fits`,
}));

const mockTargets = [
  { id: 1, name: 'M42', ra: 83.82, dec: -5.39, active: true, image_count: 10, accepted_count: 5, rejected_count: 0, has_files: true },
  { id: 2, name: 'NGC7000', ra: 314.0, dec: 44.0, active: true, image_count: 10, accepted_count: 8, rejected_count: 0, has_files: true },
];

function createWrapper(initialRoute = '/sequence?project=1&target=1') {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        gcTime: 0,
      },
    },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[initialRoute]}>
          {children}
        </MemoryRouter>
      </QueryClientProvider>
    );
  };
}

function setupDefaultHandlers() {
  server.use(
    http.get('/api/analysis/sequence', () => {
      return HttpResponse.json(normalFixture);
    }),
    http.get('/api/projects/:projectId/targets', () => {
      return HttpResponse.json({
        success: true,
        data: mockTargets,
        error: null,
        status: 'ready',
      });
    }),
    http.get('/api/images', () => {
      return HttpResponse.json({
        success: true,
        data: mockImages,
        error: null,
        status: 'ready',
      });
    }),
    http.get('/api/images/:imageId', () => {
      return HttpResponse.json({
        success: true,
        data: mockImages[0],
        error: null,
        status: 'ready',
      });
    }),
    http.put('/api/images/:imageId/grade', () => {
      return HttpResponse.json({
        success: true,
        data: null,
        error: null,
        status: 'ready',
      });
    }),
  );
}

describe('SequenceView: rendering states', () => {
  it('shows empty state when no project is selected', () => {
    render(<SequenceView />, { wrapper: createWrapper('/sequence') });
    expect(screen.getByText('Select a project to analyze image sequences')).toBeInTheDocument();
  });

  it('shows target selection when project is selected but no target', () => {
    server.use(
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1') });
    expect(screen.getByText('Sequence Analysis')).toBeInTheDocument();
    expect(screen.getByText(/Select a target/)).toBeInTheDocument();
  });

  it('shows available targets as buttons', async () => {
    server.use(
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1') });

    await waitFor(() => {
      expect(screen.getByText('M42')).toBeInTheDocument();
    });
    expect(screen.getByText('NGC7000')).toBeInTheDocument();
  });

  it('shows loading state while analyzing', async () => {
    // Use a delayed response to catch the loading state
    server.use(
      http.get('/api/analysis/sequence', async () => {
        await new Promise(resolve => setTimeout(resolve, 100));
        return HttpResponse.json(normalFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    expect(screen.getByText('Analyzing image sequences...')).toBeInTheDocument();
  });

  it('shows error state on analysis failure', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(
          { success: false, data: null, error: 'Target not found', status: null },
          { status: 400 },
        );
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText(/Failed to analyze sequence/)).toBeInTheDocument();
    });
  });

  it('shows empty state when no sequences found', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(emptyFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText(/No sequences found/)).toBeInTheDocument();
    });
  });
});

describe('SequenceView: quality display', () => {
  it('renders summary bar with quality counts', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText(/3 excellent/)).toBeInTheDocument();
    });
    expect(screen.getByText(/4 good/)).toBeInTheDocument();
    expect(screen.getByText(/3 fair/)).toBeInTheDocument();
  });

  it('renders image cards with quality badges', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      // Quality badges show percentage (e.g., "82" for 0.82)
      expect(screen.getByText('82')).toBeInTheDocument();
    });
  });

  it('shows cloud event badges when clouds are detected', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText(/cloud event/i)).toBeInTheDocument();
    });
  });

  it('shows category labels on cloud-affected images', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      // formatCategory converts "likely_clouds" to "Likely Clouds"
      const labels = screen.getAllByText('Likely Clouds');
      expect(labels.length).toBeGreaterThanOrEqual(1);
    });
  });
});

describe('SequenceView: interactions', () => {
  it('toggles image selection on click', async () => {
    setupDefaultHandlers();
    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText('82')).toBeInTheDocument();
    });

    // Find image cards and click one
    const cards = document.querySelectorAll('.sequence-image-card');
    expect(cards.length).toBeGreaterThan(0);

    await user.click(cards[0]);

    // After clicking, the card should have the 'selected' class
    expect(cards[0].classList.contains('selected')).toBe(true);

    // Click again to deselect
    await user.click(cards[0]);
    expect(cards[0].classList.contains('selected')).toBe(false);
  });

  it('selects images below threshold', async () => {
    setupDefaultHandlers();
    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText('Select Below Threshold')).toBeInTheDocument();
    });

    // Default threshold is 0.50. With the normal fixture, images below 0.50 would be none.
    // Instead, test with the default threshold -- just click the button and verify behavior.
    // The default threshold of 0.50 won't select any normal fixture images (all are >= 0.70),
    // so let's use fireEvent to set the threshold to 0.80.
    const slider = screen.getByRole('slider');
    // fireEvent is more reliable for range inputs than userEvent
    const { fireEvent } = await import('@testing-library/react');
    fireEvent.change(slider, { target: { value: '0.80' } });

    await user.click(screen.getByText('Select Below Threshold'));

    // Images with quality_score < 0.80: IDs 3 (0.75), 5 (0.70), 7 (0.77), 10 (0.72)
    // After selecting, the "Reject Selected" button should appear with count
    await waitFor(() => {
      const rejectButton = screen.queryByText(/Reject Selected/);
      expect(rejectButton).toBeInTheDocument();
    });
  });

  it('shows "Select Clouded" button and selects cloud runs', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText('Select Clouded')).toBeInTheDocument();
    });

    await user.click(screen.getByText('Select Clouded'));

    // The cloud fixture has 2 consecutive cloud images (IDs 104, 105)
    // selectCloudedSequence selects runs of >= 2 bad images
    await waitFor(() => {
      const rejectButton = screen.getByText(/Reject Selected \(2\)/);
      expect(rejectButton).toBeInTheDocument();
    });
  });

  it('clears selection when Clear button is clicked', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText('Select Clouded')).toBeInTheDocument();
    });

    // Select clouded images first
    await user.click(screen.getByText('Select Clouded'));

    await waitFor(() => {
      expect(screen.getByText(/Reject Selected/)).toBeInTheDocument();
    });

    // Click Clear
    await user.click(screen.getByText('Clear'));

    // Reject button should disappear after clearing
    await waitFor(() => {
      expect(screen.queryByText(/Reject Selected/)).not.toBeInTheDocument();
    });
  });
});

describe('SequenceView: multi-session', () => {
  it('renders sequence tabs for multiple sessions', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(multiSessionFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=2') });

    await waitFor(() => {
      // Each tab shows "filter_name (image_count)"
      const tabs = screen.getAllByText(/L \(5\)/);
      expect(tabs).toHaveLength(2);
    });
  });

  it('switches between sequences when tabs are clicked', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(multiSessionFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        });
      }),
    );

    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=2') });

    await waitFor(() => {
      const tabs = screen.getAllByText(/L \(5\)/);
      expect(tabs).toHaveLength(2);
    });

    const tabs = screen.getAllByText(/L \(5\)/);

    // First tab should be active by default
    expect(tabs[0].classList.contains('active')).toBe(true);

    // Click second tab
    await user.click(tabs[1]);

    // Second tab should now be active
    expect(tabs[1].classList.contains('active')).toBe(true);
    expect(tabs[0].classList.contains('active')).toBe(false);
  });
});

describe('SequenceView: batch operations', () => {
  it('calls grade API when rejecting selected images', async () => {
    server.use(
      http.get('/api/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/images/:imageId', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages[0],
          error: null,
          status: 'ready',
        });
      }),
    );

    const gradeRequests: Array<{ imageId: string; body: unknown }> = [];
    server.use(
      http.put('/api/images/:imageId/grade', async ({ params, request }) => {
        const body = await request.json();
        gradeRequests.push({ imageId: params.imageId as string, body });
        return HttpResponse.json({
          success: true,
          data: null,
          error: null,
          status: 'ready',
        });
      }),
    );

    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText('Select Clouded')).toBeInTheDocument();
    });

    // Select clouded images
    await user.click(screen.getByText('Select Clouded'));

    await waitFor(() => {
      expect(screen.getByText(/Reject Selected \(2\)/)).toBeInTheDocument();
    });

    // Click reject
    await user.click(screen.getByText(/Reject Selected \(2\)/));

    // Wait for the grade API calls to be made
    await waitFor(() => {
      expect(gradeRequests.length).toBe(2);
    });

    // Verify the grade requests were for rejection
    gradeRequests.forEach(req => {
      expect((req.body as Record<string, unknown>).status).toBe('rejected');
    });
  });

  it('shows Re-analyze button', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText('Re-analyze')).toBeInTheDocument();
    });
  });
});
