import { describe, it, expect } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import SequenceView from '../SequenceView';
import normalFixture from '../../__fixtures__/sequence-analysis-normal.json';
import cloudsFixture from '../../__fixtures__/sequence-analysis-clouds.json';
import multiSessionFixture from '../../__fixtures__/sequence-analysis-multi-session.json';
import emptyFixture from '../../__fixtures__/sequence-analysis-empty.json';

const multiSessionRollup = {
  ...multiSessionFixture.data.sequences[0],
  session_end: multiSessionFixture.data.sequences[1].session_end,
  image_count: 10,
  reference_values: {
    best_star_count: null,
    best_hfr: null,
    best_eccentricity: null,
    best_snr: null,
    best_background: null,
  },
  images: [
    ...multiSessionFixture.data.sequences[0].images.map(image => (
      image.image_id === 201 ? { ...image, quality_score: 0.42 } : image
    )),
    ...multiSessionFixture.data.sequences[1].images,
  ],
  summary: {
    excellent_count: 4,
    good_count: 5,
    fair_count: 0,
    poor_count: 1,
    bad_count: 0,
    cloud_events_detected: 0,
    focus_drift_detected: false,
    tracking_issues_detected: false,
    out_of_target_count: 0,
    plate_solve_failed_count: 0,
    satellite_risk_count: 0,
  },
};

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

function createWrapper(initialRoute = '/sequence?db=test&project=1&target=1') {
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
    http.get('/api/db/:dbId/analysis/sequence', () => {
      return HttpResponse.json(normalFixture);
    }),
    http.get('/api/db/:dbId/projects/:projectId/targets', () => {
      return HttpResponse.json({
        success: true,
        data: mockTargets,
        error: null,
        status: 'ready',
      });
    }),
    http.get('/api/db/:dbId/images', () => {
      return HttpResponse.json({
        success: true,
        data: mockImages,
        error: null,
        status: 'ready',
      });
    }),
    http.get('/api/db/:dbId/images/:imageId', () => {
      return HttpResponse.json({
        success: true,
        data: mockImages[0],
        error: null,
        status: 'ready',
      });
    }),
    http.put('/api/db/:dbId/images/:imageId/grade', () => {
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
    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test') });
    expect(screen.getByText('Select a project to analyze image sequences')).toBeInTheDocument();
  });

  it('shows target selection when project is selected but no target', () => {
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1') });
    expect(screen.getByText('Sequence Analysis')).toBeInTheDocument();
    expect(screen.getByText(/Select a target/)).toBeInTheDocument();
  });

  it('shows available targets as buttons', async () => {
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1') });

    await waitFor(() => {
      expect(screen.getByText('M42')).toBeInTheDocument();
    });
    expect(screen.getByText('NGC7000')).toBeInTheDocument();
  });

  it('opens the only target in a project', async () => {
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: [mockTargets[0]],
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(normalFixture);
      }),
    );
    let search = '';
    function LocationProbe() {
      search = useLocation().search;
      return null;
    }

    const Wrapper = createWrapper('/sequence?db=test&project=1');
    render(
      <Wrapper>
        <SequenceView />
        <LocationProbe />
      </Wrapper>
    );

    await waitFor(() => {
      expect(new URLSearchParams(search).get('target')).toBe('1');
    });
    expect(await screen.findByText('82')).toBeInTheDocument();
  });

  it('uses the current Grid image to choose a target', async () => {
    const selectedImageId = multiSessionFixture.data.sequences[1].images[1].image_id;
    const selectedImage = {
      ...mockImages[0],
      id: selectedImageId,
      target_id: 2,
      target_name: 'NGC7000',
    };
    server.use(
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: [...mockImages, selectedImage],
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(multiSessionFixture);
      }),
    );
    let search = '';
    function LocationProbe() {
      search = useLocation().search;
      return null;
    }

    const Wrapper = createWrapper(`/sequence?db=test&project=1&current=${selectedImageId}`);
    render(
      <Wrapper>
        <SequenceView />
        <LocationProbe />
      </Wrapper>
    );

    await waitFor(() => {
      expect(new URLSearchParams(search).get('target')).toBe('2');
    });
    expect(new URLSearchParams(search).get('current')).toBe(String(selectedImageId));
    const tabs = await screen.findAllByRole('button', { name: /L · .* \(5\)/ });
    expect(tabs[1]).toHaveClass('active');
    expect(document.querySelector(`[data-card-image-id="${selectedImageId}"]`)).toHaveClass(
      'current-selection'
    );
  });

  it('clicking a target card keeps the db/project context (no blank analysis)', async () => {
    setupDefaultHandlers();

    // Probe the live router location so we can assert the URL after navigation.
    let search = '';
    function LocationProbe() {
      search = useLocation().search;
      return null;
    }

    const Wrapper = createWrapper('/sequence?db=test&project=1');
    render(
      <Wrapper>
        <SequenceView />
        <LocationProbe />
      </Wrapper>
    );

    // From the target-selection screen, click into a target's analysis.
    const card = await screen.findByText('M42');
    await userEvent.click(card);

    // The db slug (and project/target) must survive the navigation — dropping
    // ?db= is what stranded the user on a blank analysis view.
    await waitFor(() => {
      const params = new URLSearchParams(search);
      expect(params.get('db')).toBe('test');
      expect(params.get('project')).toBe('1');
      expect(params.get('target')).toBe('1'); // M42 = id 1
    });
  });

  it('shows loading state while analyzing', async () => {
    // Use a delayed response to catch the loading state
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', async () => {
        await new Promise(resolve => setTimeout(resolve, 100));
        return HttpResponse.json(normalFixture);
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    expect(screen.getByText('Analyzing image sequences...')).toBeInTheDocument();
  });

  it('shows error state on analysis failure', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(
          { success: false, data: null, error: 'Target not found', status: null },
          { status: 400 },
        );
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText(/Failed to analyze sequence/)).toBeInTheDocument();
    });
  });

  it('shows empty state when no sequences found', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(emptyFixture);
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText(/No sequences found/)).toBeInTheDocument();
    });
  });
});

describe('SequenceView: quality display', () => {
  it('renders summary bar with quality counts', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText('3 at 90–100')).toBeInTheDocument();
    }, { timeout: 3000 });
    expect(screen.getByText('4 at 70–89')).toBeInTheDocument();
    expect(screen.getByText('3 at 50–69')).toBeInTheDocument();
  }, 10000);

  it('renders image cards with quality badges', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      // Quality badges show percentage (e.g., "82" for 0.82)
      expect(screen.getByText('82')).toBeInTheDocument();
    });
  });

  it('renders timeline text without non-uniform SVG scaling', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    const timeline = await screen.findByRole('img', { name: 'Capture sequence comparison scores' });
    expect(timeline).not.toHaveAttribute('preserveAspectRatio', 'none');
    expect(timeline).toHaveAttribute('width', '400');
    expect(timeline).toHaveAttribute('height', '120');
  });

  it('shows cloud event badges when clouds are detected', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText(/cloud event/i)).toBeInTheDocument();
    });
  });

  it('shows category labels on cloud-affected images', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

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

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

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

  it('changes thumbnail size and keeps it in the URL', async () => {
    setupDefaultHandlers();
    let search = '';
    function LocationProbe() {
      search = useLocation().search;
      return null;
    }

    const Wrapper = createWrapper('/sequence?db=test&project=1&target=1');
    render(
      <Wrapper>
        <SequenceView />
        <LocationProbe />
      </Wrapper>
    );

    await screen.findByText('82');
    const size = screen.getByLabelText('Size:');
    expect(size).toHaveValue('150');

    fireEvent.change(size, { target: { value: '500' } });

    await waitFor(() => {
      expect(new URLSearchParams(search).get('size')).toBe('500');
    });
    expect(document.querySelector('.sequence-strip')).toHaveStyle({
      gridTemplateColumns: 'repeat(auto-fill, minmax(min(500px, 100%), 1fr))',
    });
    expect(screen.getByText('500px')).toBeInTheDocument();
  });

  it('moves the cursor with arrows and toggles selection with Space', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(document.querySelectorAll('.sequence-image-card')).toHaveLength(10);
    });
    const cards = document.querySelectorAll('.sequence-image-card');
    await waitFor(() => expect(cards[0]).toHaveClass('current-selection'));

    fireEvent.keyDown(document, { key: ' ', code: 'Space' });
    expect(cards[0]).toHaveClass('selected');

    fireEvent.keyDown(document, { key: 'ArrowRight', code: 'ArrowRight' });
    await waitFor(() => expect(cards[1]).toHaveClass('current-selection'));
    expect(cards[0]).toHaveClass('selected');

    fireEvent.keyDown(document, { key: ' ', code: 'Space' });
    expect(cards[1]).toHaveClass('selected');

    fireEvent.keyDown(document, { key: 'ArrowLeft', code: 'ArrowLeft' });
    await waitFor(() => expect(cards[0]).toHaveClass('current-selection'));
  });

  it('selects a range with Shift-click', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(document.querySelectorAll('.sequence-image-card')).toHaveLength(10);
    });
    const cards = document.querySelectorAll('.sequence-image-card');
    fireEvent.click(cards[2], { shiftKey: true });

    await waitFor(() => {
      expect(document.querySelectorAll('.sequence-image-card.selected')).toHaveLength(3);
    });
    expect(cards[0]).toHaveClass('selected');
    expect(cards[1]).toHaveClass('selected');
    expect(cards[2]).toHaveClass('selected');

    fireEvent.click(cards[1], { shiftKey: true });
    await waitFor(() => {
      expect(document.querySelectorAll('.sequence-image-card.selected')).toHaveLength(2);
    });
    expect(cards[2]).not.toHaveClass('selected');
  });

  it('selects a range with Shift-click in the quality chart', async () => {
    setupDefaultHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(document.querySelectorAll('.sequence-timeline rect[data-image-id]')).toHaveLength(10);
    });
    const bars = document.querySelectorAll('.sequence-timeline rect[data-image-id]');
    fireEvent.click(bars[0]);
    fireEvent.click(bars[2], { shiftKey: true });

    await waitFor(() => {
      expect(document.querySelectorAll('.sequence-image-card.selected')).toHaveLength(3);
    });
  });

  it('selects images below threshold', async () => {
    setupDefaultHandlers();
    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByLabelText('Select:')).toBeInTheDocument();
    });

    // Default threshold is 0.50. With the normal fixture, images below 0.50 would be none.
    // Instead, test with the default threshold -- just click the button and verify behavior.
    // The default threshold of 0.50 won't select any normal fixture images (all are >= 0.70),
    // so let's use fireEvent to set the threshold to 0.80.
    const slider = screen.getByLabelText('Score threshold:');
    // fireEvent is more reliable for range inputs than userEvent
    const { fireEvent } = await import('@testing-library/react');
    fireEvent.change(slider, { target: { value: '0.80' } });

    await user.selectOptions(screen.getByLabelText('Select:'), 'threshold');

    // Images with quality_score < 0.80: IDs 3 (0.75), 5 (0.70), 7 (0.77), 10 (0.72)
    // After selecting, the review action should appear with the selected count.
    await waitFor(() => {
      expect(screen.getByText('4 selected')).toBeInTheDocument();
      expect(screen.getByText('Review rejection')).toBeInTheDocument();
    });
  });

  it('uses a selection preset as the base for the next Shift-click range', async () => {
    setupDefaultHandlers();
    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(document.querySelectorAll('.sequence-image-card')).toHaveLength(10);
    });
    const cards = document.querySelectorAll('.sequence-image-card');
    fireEvent.click(cards[0]);
    fireEvent.click(cards[8]);

    fireEvent.change(screen.getByLabelText('Score threshold:'), { target: { value: '0.80' } });
    await user.selectOptions(screen.getByLabelText('Select:'), 'threshold');
    await waitFor(() => expect(screen.getByText('4 selected')).toBeInTheDocument());

    fireEvent.click(cards[9], { shiftKey: true });

    await waitFor(() => expect(screen.getByText('5 selected')).toBeInTheDocument());
    expect(cards[0]).not.toHaveClass('selected');
    expect(cards[2]).toHaveClass('selected');
    expect(cards[4]).toHaveClass('selected');
    expect(cards[6]).toHaveClass('selected');
    expect(cards[8]).toHaveClass('selected');
    expect(cards[9]).toHaveClass('selected');
  });

  it('shows "Select Clouded" button and selects cloud runs', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByLabelText('Select:')).toBeInTheDocument();
    });

    await user.selectOptions(screen.getByLabelText('Select:'), 'clouded');

    // The cloud fixture has 2 consecutive cloud images (IDs 104, 105)
    // selectCloudedSequence selects runs of >= 2 bad images
    await waitFor(() => {
      expect(screen.getByText('2 selected')).toBeInTheDocument();
      expect(screen.getByText('Review rejection')).toBeInTheDocument();
    });
  });

  it('clears selection when Clear button is clicked', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
    );

    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByLabelText('Select:')).toBeInTheDocument();
    });

    // Select clouded images first
    await user.selectOptions(screen.getByLabelText('Select:'), 'clouded');

    await waitFor(() => {
      expect(screen.getByText('Review rejection')).toBeInTheDocument();
    });

    // Click Clear
    await user.click(screen.getByText('Clear'));

    // Reject button should disappear after clearing
    await waitFor(() => {
      expect(screen.queryByText('Review rejection')).not.toBeInTheDocument();
    });
  });
});

describe('SequenceView: multi-session', () => {
  function setupMultiSessionHandlers() {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json({
          ...multiSessionFixture,
          data: {
            ...multiSessionFixture.data,
            target_filter_rollups: [multiSessionRollup],
          },
        });
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: [],
          error: null,
          status: 'ready',
        });
      }),
    );
  }

  it('renders sequence tabs for multiple sessions', async () => {
    setupMultiSessionHandlers();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=2') });

    await waitFor(() => {
      // Each tab shows the filter, session time, and image count.
      const tabs = screen.getAllByRole('button', { name: /L · .* \(5\)/ });
      expect(tabs).toHaveLength(2);
      const year = new Date(
        multiSessionFixture.data.sequences[0].session_start * 1000,
      ).getFullYear();
      expect(tabs[0]).toHaveTextContent(String(year));
    });
    expect(screen.getByRole('button', { name: 'L · All sessions (10)' })).toHaveClass('active');
  });

  it('compares all stack candidates without replacing session scores', async () => {
    setupMultiSessionHandlers();
    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=2') });

    const rollupTab = await screen.findByRole('button', { name: 'L · All sessions (10)' });
    expect(rollupTab).toHaveClass('active');
    expect(document.querySelectorAll('.sequence-image-card')).toHaveLength(10);
    expect(screen.getByText('Stack comparison · matching capture settings across all sessions')).toBeInTheDocument();
    expect(document.querySelector('[data-card-image-id="201"] .quality-badge')).toHaveTextContent('42');

    await user.selectOptions(screen.getByLabelText('Select:'), 'threshold');
    await user.click(screen.getByRole('button', { name: 'Review rejection' }));
    expect(screen.getByRole('dialog')).toHaveTextContent('Image 201 · score 0.42');
    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    const sessionTabs = screen.getAllByRole('button', { name: /L · .* \(5\)/ });
    await user.click(sessionTabs[0]);
    expect(document.querySelectorAll('.sequence-image-card')).toHaveLength(5);
    expect(document.querySelector('[data-card-image-id="201"] .quality-badge')).toHaveTextContent('78');
    expect(screen.getByText('Session comparison · one capture run')).toBeInTheDocument();
  });

  it('switches between sequences when tabs are clicked', async () => {
    setupMultiSessionHandlers();

    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=2') });

    await waitFor(() => {
      const tabs = screen.getAllByRole('button', { name: /L · .* \(5\)/ });
      expect(tabs).toHaveLength(2);
    });

    const tabs = screen.getAllByRole('button', { name: /L · .* \(5\)/ });

    // The all-session stack comparison is active by default.
    expect(screen.getByRole('button', { name: 'L · All sessions (10)' })).toHaveClass('active');
    expect(tabs[0].classList.contains('active')).toBe(false);

    // Click second tab
    await user.click(tabs[1]);

    // Second tab should now be active
    expect(tabs[1].classList.contains('active')).toBe(true);
    expect(tabs[0].classList.contains('active')).toBe(false);
  });

  it('keeps cross-session selections visible and reviews all of them', async () => {
    setupMultiSessionHandlers();
    const gradedImageIds: string[] = [];
    server.use(
      http.put('/api/db/:dbId/images/:imageId/grade', ({ params }) => {
        gradedImageIds.push(params.imageId as string);
        return HttpResponse.json({
          success: true,
          data: null,
          error: null,
          status: 'ready',
        });
      }),
    );
    const user = userEvent.setup();

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=2') });

    const tabs = await screen.findAllByRole('button', { name: /L · .* \(5\)/ });
    let cards = document.querySelectorAll('.sequence-image-card');
    await user.click(cards[0]);
    expect(tabs[0].querySelector('.sequence-tab-selection-count')).toHaveTextContent('1');

    await user.click(tabs[1]);
    cards = document.querySelectorAll('.sequence-image-card');
    await user.click(cards[0]);

    expect(document.querySelectorAll('.sequence-tab-selection-count')).toHaveLength(3);
    expect(screen.getByText('2 selected')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Review rejection' }));
    await user.click(screen.getByRole('button', { name: 'Reject selected (2)' }));
    await waitFor(() => expect(gradedImageIds).toHaveLength(2));
    expect(gradedImageIds).toEqual(expect.arrayContaining([
      String(multiSessionFixture.data.sequences[0].images[0].image_id),
      String(multiSessionFixture.data.sequences[1].images[0].image_id),
    ]));
  });

  it('restores a URL selection across session tabs', async () => {
    setupMultiSessionHandlers();
    const firstImageId = multiSessionFixture.data.sequences[0].images[0].image_id;
    const secondImageId = multiSessionFixture.data.sequences[1].images[0].image_id;

    render(<SequenceView />, {
      wrapper: createWrapper(
        `/sequence?db=test&project=1&target=2&current=${secondImageId}`
        + `&selected=${firstImageId},${secondImageId}`
      ),
    });

    await screen.findAllByRole('button', { name: /L · .* \(5\)/ });
    expect(document.querySelectorAll('.sequence-tab-selection-count')).toHaveLength(3);
    expect(screen.getByText('2 selected')).toBeInTheDocument();
    expect(document.querySelector('.sequence-image-card.selected')).toHaveAttribute(
      'data-card-image-id',
      String(secondImageId),
    );
  });

  it('stores the active image and return view when opening Detail', async () => {
    setupMultiSessionHandlers();
    const user = userEvent.setup();
    let pathname = '';
    let search = '';
    function LocationProbe() {
      const location = useLocation();
      pathname = location.pathname;
      search = location.search;
      return null;
    }

    const Wrapper = createWrapper('/sequence?db=test&project=1&target=2');
    render(
      <Wrapper>
        <SequenceView />
        <LocationProbe />
      </Wrapper>
    );

    const tabs = await screen.findAllByRole('button', { name: /L · .* \(5\)/ });
    await user.click(tabs[1]);

    await waitFor(() => {
      const params = new URLSearchParams(search);
      expect(params.get('current')).toBe('211');
    });

    const cards = document.querySelectorAll('.sequence-image-card');
    expect(cards).toHaveLength(5);
    await user.dblClick(cards[1]);

    await waitFor(() => {
      const params = new URLSearchParams(search);
      expect(pathname).toBe('/detail/212');
      expect(params.get('returnTo')).toBe('sequence');
      expect(params.get('current')).toBe('212');
    });
  });

  it('restores a session from URL state', async () => {
    setupMultiSessionHandlers();

    render(<SequenceView />, {
      wrapper: createWrapper(
        '/sequence?db=test&project=1&target=2&current=212'
      ),
    });

    const tabs = await screen.findAllByRole('button', { name: /L · .* \(5\)/ });
    expect(tabs[1]).toHaveClass('active');
    expect(document.querySelectorAll('.sequence-image-card')[1]).toHaveClass(
      'current-selection'
    );
  });
});

describe('SequenceView: batch operations', () => {
  it('calls grade API when rejecting selected images', async () => {
    server.use(
      http.get('/api/db/:dbId/analysis/sequence', () => {
        return HttpResponse.json(cloudsFixture);
      }),
      http.get('/api/db/:dbId/projects/:projectId/targets', () => {
        return HttpResponse.json({
          success: true,
          data: mockTargets,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images', () => {
        return HttpResponse.json({
          success: true,
          data: mockImages,
          error: null,
          status: 'ready',
        });
      }),
      http.get('/api/db/:dbId/images/:imageId', () => {
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
      http.put('/api/db/:dbId/images/:imageId/grade', async ({ params, request }) => {
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

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByLabelText('Select:')).toBeInTheDocument();
    });

    // Select clouded images
    await user.selectOptions(screen.getByLabelText('Select:'), 'clouded');

    await waitFor(() => {
      expect(screen.getByText('2 selected')).toBeInTheDocument();
    });

    // Open review, verify that no grade has been written yet, then confirm.
    await user.click(screen.getByText('Review rejection'));
    expect(screen.getByRole('dialog', { name: /Review 2 selected frames/ })).toBeInTheDocument();
    expect(gradeRequests).toHaveLength(0);
    await user.click(screen.getByText(/Reject selected \(2\)/));

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

    render(<SequenceView />, { wrapper: createWrapper('/sequence?db=test&project=1&target=1') });

    await waitFor(() => {
      expect(screen.getByText('Re-analyze')).toBeInTheDocument();
    });
  });
});
