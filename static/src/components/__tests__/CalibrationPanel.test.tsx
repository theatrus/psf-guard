import { beforeEach, describe, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation, useNavigate } from 'react-router-dom';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import CalibrationPanel from '../CalibrationPanel';
import { setDetailSection, setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import type {
  CalibrationKindExplanation,
  CalibrationLightMark,
  ImageCalibrationExplanation,
} from '../../api/types';

function kind(
  name: CalibrationKindExplanation['kind'],
  extra: Partial<CalibrationKindExplanation> = {},
): CalibrationKindExplanation {
  return {
    kind: name,
    master: { frames: 20, nights: 1, temp_min_c: -10, temp_max_c: -10, first_night: '2026-09-28', last_night: '2026-09-28' },
    used: [{ night: '2026-09-28', frames: 20, temp_min_c: -10, temp_max_c: -10 }],
    matching_unused: [],
    refused: [],
    kept_out: [],
    suspect: [],
    other_filters: 0,
    other_cameras: 0,
    ...extra,
  };
}

const explanation: ImageCalibrationExplanation = {
  light: {
    file: 'light.fits', night: '2026-10-06', filter: 'Ha', exposure_s: 300, gain: 100, offset: 50,
    binning: '1x1', camera_temp: -10, rotator_deg: null, camera: 'Cam', telescope: null, focal_length_mm: null,
  },
  findings: [
    { code: 'thin_dark', level: 'poor', reason: 'Its dark master would use only 3 frames', kinds: ['dark'] },
    { code: 'mixed_flats', level: 'check', reason: 'Its flat master mixes flats from 2 nights', kinds: ['flat'] },
  ],
  kinds: [
    kind('bias'),
    kind('dark', { master: { frames: 3, nights: 1 }, used: [{ night: '2026-09-28', frames: 3 }] }),
    kind('dark_flat', { master: { frames: 0, nights: 0 }, used: [] }),
    kind('flat', {
      master: { frames: 46, nights: 2 },
      used: [
        { night: '2026-10-06', frames: 6 },
        { night: '2026-10-01', frames: 40 },
      ],
    }),
  ],
};

function Where() {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <>
      <output data-testid="where">{location.search}</output>
      {/* Stands in for a chip or a filter changing the address. */}
      <input aria-label="go" onChange={(event) => navigate(event.target.value)} />
    </>
  );
}

const thinDark: CalibrationLightMark = { image_id: 8, level: 'poor', codes: ['thin_dark', 'mixed_flats'] };

/** Lights asked about, in order. */
let asked: number[] = [];

function show(
  route: string,
  data: ImageCalibrationExplanation = explanation,
  marks: CalibrationLightMark[] = [],
) {
  asked = [];
  server.use(
    http.get('/api/db/:dbId/images/:imageId/calibration', ({ params }) => {
      asked.push(Number(params.imageId));
      return HttpResponse.json({ success: true, error: null, status: 'ready', data });
    }),
    http.get('/api/db/:dbId/calibration-health/lights', () =>
      HttpResponse.json({
        success: true, error: null, status: 'ready',
        data: { lights: marks, stale_projects: [], running: false },
      })),
  );
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[route]}>
        {children}
        <Where />
      </MemoryRouter>
    </QueryClientProvider>
  );
  const view = render(<CalibrationPanel dbId="c925" imageId={8} projectId={1} />, { wrapper: Wrapper });
  return {
    /** Arrow-key navigation: the same panel, another light. */
    showImage: (imageId: number) =>
      view.rerender(<CalibrationPanel dbId="c925" imageId={imageId} projectId={1} />),
  };
}

const focus = () => new URLSearchParams(screen.getByTestId('where').textContent!).get('calibFocus');
const heading = () => screen.getByRole('button', { name: /^Calibration/ });
const go = (route: string) => fireEvent.change(screen.getByLabelText('go'), { target: { value: route } });

describe('CalibrationPanel', () => {
  beforeEach(() => {
    setDisplayPreferences({
      showNightChip: true,
      showAllChip: true,
      showCalibrationChip: true,
      advanceOnGrade: true,
      projectPickerGrouping: 'activity',
      libraryDensity: 'compact',
      detailSections: {},
    });
  });

  it('opens on the finding Images was filtered by and picks out its nights', async () => {
    show('/detail/8?db=c925&project=1&calib=mixed_flats');
    const mixed = await screen.findByRole('button', { name: /mixes flats from 2 nights/ });
    expect(mixed).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: /only 3 frames/ })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByText(/One master averages flats from nights not measured against each other/)).toBeInTheDocument();

    const flats = screen.getByTestId('calibration-kind-flat');
    expect(flats).toHaveClass('is-about', 'is-check');
    const nights = within(flats).getAllByRole('listitem');
    expect(nights.map((night) => night.textContent)).toEqual([
      "2026-10-066 framesthe light's night",
      '2026-10-0140 frames5 days from the light',
    ]);
    expect(nights.every((night) => night.classList.contains('is-focus'))).toBe(true);
    // The kinds it is not about fold to one line; one with nothing is left out.
    expect(screen.getByTestId('calibration-kind-dark')).toHaveClass('is-folded');
    expect(within(screen.getByTestId('calibration-kind-dark')).queryByRole('listitem')).toBeNull();
    expect(screen.queryByTestId('calibration-kind-dark_flat')).toBeNull();

    // A night opens its frames on the Calibration page, without the detail's focus.
    const link = within(nights[1]).getByRole('link', { name: '2026-10-01' });
    const href = new URL(link.getAttribute('href')!, 'http://x');
    expect(href.pathname).toBe('/calibration');
    expect(Object.fromEntries(href.searchParams)).toMatchObject({
      db: 'c925', project: '1', calKind: 'flat', calFilter: 'Ha', calFrom: '2026-10-01', calTo: '2026-10-01',
    });
    expect(href.searchParams.get('calibFocus')).toBeNull();
    // Frames a light matched or was refused lie outside its project's
    // scope, so the link opens the rig's whole library.
    expect(href.searchParams.has('calProject')).toBe(false);
    const opens = screen.getAllByRole('link', { name: 'Open' });
    expect(opens.length).toBeGreaterThan(0);
    for (const kindLink of opens) {
      expect(new URL(kindLink.getAttribute('href')!, 'http://x').searchParams.has('calProject')).toBe(false);
    }
  });

  it('keeps the chosen finding in the URL and can show every kind', async () => {
    // Marked, so the panel opens without a purpose.
    show('/detail/8?db=c925&project=1', explanation, [thinDark]);
    // With nothing chosen, the worst finding shows.
    const thin = await screen.findByRole('button', { name: /only 3 frames/ });
    expect(thin).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('calibration-kind-dark')).toHaveClass('is-about', 'is-poor');
    expect(screen.getByTestId('calibration-kind-flat')).toHaveClass('is-folded');

    fireEvent.click(screen.getByRole('button', { name: /mixes flats from 2 nights/ }));
    await waitFor(() => expect(focus()).toBe('mixed_flats'));
    expect(screen.getByTestId('calibration-kind-dark')).toHaveClass('is-folded');

    fireEvent.click(screen.getByRole('button', { name: 'Show every kind' }));
    await waitFor(() => expect(focus()).toBe('none'));
    expect(document.querySelectorAll('.light-calibration-kind.is-folded')).toHaveLength(0);
    expect(screen.getByText(/Choose a finding/)).toBeInTheDocument();

    // Choosing the same finding again lets it go.
    fireEvent.click(screen.getByRole('button', { name: /only 3 frames/ }));
    await waitFor(() => expect(focus()).toBe('thin_dark'));
    fireEvent.click(screen.getByRole('button', { name: /only 3 frames/ }));
    await waitFor(() => expect(focus()).toBe('none'));
  });

  it('says how its flats were chosen around a measured change, and how alike its nights measure', async () => {
    const event = {
      key: 'e1', start: 1759600000, end: 1759700000, dust: 0.016, broad: 0.002, camera_side: true,
      filters: ['Ha', 'OIII'], what: 'new dust: 2 shadows near (1418, 3540)',
    };
    show('/detail/8?db=c925&project=1&calib=flats_differ', {
      ...explanation,
      findings: [{
        code: 'flats_differ', level: 'check', kinds: ['flat'],
        reason: 'New dust came between this light and every flat that matches it',
      }],
      kinds: [kind('flat', {
        master: { frames: 46, nights: 2 },
        used: [{ night: '2026-10-06', frames: 6 }, { night: '2026-10-01', frames: 40 }],
        matching_unused: [{ night: '2026-10-08', frames: 20 }],
        alike: { nights: 2, dust: 0.00004, broad: 0.0064 },
        chosen_by: 'No flats from its side of the new dust; these are from before it',
        basis: { basis: 'no_same_side', event, placement: { by: 'time', side: 'after' } },
      })],
    });
    const finding = await screen.findByRole('button', { name: /New dust came between/ });
    expect(finding).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByText(/A measured change, such as new dust, lies between this light and its flats/)).toBeInTheDocument();
    expect(screen.getByTestId('calibration-flat-alike'))
      .toHaveTextContent('Measured alike: dust within <0.01%, pattern within 0.64% across 2 nights');
    const chosenBy = screen.getByTestId('calibration-flat-chosen-by');
    expect(chosenBy).toHaveTextContent('Chosen by');
    expect(chosenBy).toHaveTextContent('No flats from its side of the new dust');
    // Flats from across a change: worth a check.
    expect(chosenBy).toHaveClass('is-check');
    // The finding picks out both sides' nights: the ones taken and the ones left.
    const flats = screen.getByTestId('calibration-kind-flat');
    expect(flats.querySelector('.light-calibration-kind-heading')).toHaveClass('is-focus');
    const nights = within(flats).getAllByRole('listitem');
    expect(nights).toHaveLength(3);
    expect(nights.every((night) => night.classList.contains('is-focus'))).toBe(true);
  });

  it('reads flats taken from their own side of a change as a plain note', async () => {
    const event = {
      key: 'e1', start: 1759600000, end: 1759700000, dust: 0.016, broad: 0.002, camera_side: true,
      filters: ['Ha'], what: 'new dust',
    };
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [], kinds: [kind('flat', {
      chosen_by: "The light's own side of the new dust",
      basis: { basis: 'same_side', event, placement: { by: 'time', side: 'after' }, left_out: 20 },
    })] });
    fireEvent.click(await screen.findByRole('button', { name: /^Calibration/ }));
    const chosenBy = await screen.findByTestId('calibration-flat-chosen-by');
    expect(chosenBy).toHaveTextContent("The light's own side of the new dust");
    expect(chosenBy).not.toHaveClass('is-check');
    expect(screen.queryByTestId('calibration-flat-alike')).toBeNull();
  });

  it('names the flat family its own pixels chose', async () => {
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [], kinds: [kind('flat', {
      chosen_by: 'Selected based on pixel analysis: lights from this imaging session (8 of M 27) match '
        + 'the OIII at 72.6° & 82.2° flat family at the recorded angle, rather than OIII at 100.1–104.4° (77.0° recorded)',
      family: {
        key: 'OIII:s-oiii72', label: 'OIII at 72.6° & 82.2°', by: 'pixels', lights: 8, targets: ['M 27'],
        moved: false, at_recorded: true,
        light_angle: 77, shift: 0, candidates: ['OIII at 72.6° & 82.2°', 'OIII at 100.1–104.4°'],
      },
    })] });
    fireEvent.click(await screen.findByRole('button', { name: /^Calibration/ }));
    const family = await screen.findByTestId('calibration-flat-family');
    expect(family).toHaveTextContent('Flat family');
    expect(family).toHaveTextContent('OIII at 72.6° & 82.2°');
    expect(family).toHaveTextContent('selected based on pixel analysis');
    expect(screen.getByTestId('calibration-flat-chosen-by')).toHaveTextContent('lights from this imaging session (8 of M 27) match');
  });

  it('says over which span its family measures alike when its flats lie at another angle', async () => {
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [], kinds: [kind('flat', {
      chosen_by: 'Using flats at 104.0°: measured responses match across 101.5–102.2° and 104.2–104.4° '
        + '(10 pairs over 9 nights). Stack calibration uses 104.0° as the matching angle for this light',
      family: {
        key: 'OIII:s-oiii104', label: 'OIII at 101.3–104.4°', by: 'angle', at_recorded: true,
        light_angle: 100.9, shift: 0, candidates: ['OIII at 101.3–104.4°'],
        alike_range: {
          flats_angle: 104, low: 101.5, high: 104.4, label: '101.5–102.2° and 104.2–104.4°', pairs: 10, nights: 9,
          carried: true,
        },
      },
    })] });
    fireEvent.click(await screen.findByRole('button', { name: /^Calibration/ }));
    expect(await screen.findByTestId('calibration-flat-alike-range'))
      .toHaveTextContent('flats at 104.0°, measured alike at 101.5–102.2° and 104.2–104.4° (10 pairs)');
    expect(screen.getByTestId('calibration-flat-chosen-by'))
      .toHaveTextContent('measured responses match across 101.5–102.2° and 104.2–104.4°');
  });

  it('names what confirmed a move its own pixels made', async () => {
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [], kinds: [kind('flat', {
      family: {
        key: 'Ha:s-ha8', label: 'Ha at 8.0°', by: 'pixels', lights: 5, targets: ['M 27'],
        moved: true, corroborated: 'its OIII lights', at_recorded: false,
        light_angle: 4, shift: 0, candidates: ['Ha at 8.0°', 'Ha at 0.0°'],
      },
    })] });
    fireEvent.click(await screen.findByRole('button', { name: /^Calibration/ }));
    expect(await screen.findByTestId('calibration-flat-family'))
      .toHaveTextContent('reassigned based on pixel analysis, confirmed by its OIII lights');
  });

  it('says when no flat family stood clear and the recorded angle was kept', async () => {
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [], kinds: [kind('flat', {
      family: {
        key: 'OIII:s-oiii72', label: 'OIII at 82.2°', by: 'nearest', undecided: true, at_recorded: true,
        light_angle: 82, shift: 0, candidates: ['OIII at 82.2°', 'OIII at 72.6°'],
      },
    })] });
    fireEvent.click(await screen.findByRole('button', { name: /^Calibration/ }));
    expect(await screen.findByTestId('calibration-flat-family'))
      .toHaveTextContent('uses the recorded angle: pixel analysis was inconclusive');
  });

  it('marks a move its own pixels made that nothing confirmed as a check', async () => {
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [], kinds: [kind('flat', {
      chosen_by: 'Its imaging session\'s lights (5 of M 27) match the flats of the Ha at 8.0° family',
      family: {
        key: 'Ha:s-ha0', label: 'Ha at 0.0°', by: 'disputed', lights: 5, targets: ['M 27'],
        pointed: 'Ha at 8.0°', at_recorded: true,
        light_angle: 0.5, shift: 0, candidates: ['Ha at 0.0°', 'Ha at 8.0°'],
      },
    })] });
    fireEvent.click(await screen.findByRole('button', { name: /^Calibration/ }));
    expect(await screen.findByTestId('calibration-flat-family'))
      .toHaveTextContent('check: pixel analysis suggests Ha at 8.0°, unconfirmed');
    expect(screen.getByTestId('calibration-flat-chosen-by')).toHaveClass('is-check');
  });

  it('says so when the library has no finding for the light', async () => {
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [] });
    // No mark: folded, and it says why without asking for the account.
    await waitFor(() => expect(heading()).toHaveAccessibleName('Calibration no findings'));
    expect(heading()).toHaveAttribute('aria-expanded', 'false');
    expect(asked).toEqual([]);

    fireEvent.click(heading());
    expect(await screen.findByText(/calibrates this light with no findings/)).toBeInTheDocument();
    expect(screen.getByText('No findings')).toBeInTheDocument();
    expect(document.querySelectorAll('.light-calibration-kind.is-folded')).toHaveLength(0);
    expect(asked).toEqual([8]);
  });

  it('shows no Check once the suspect frame its master takes is used anyway', async () => {
    const suspectFlat = { file: 'flat-1.fits', night: '2026-10-06', reason: 'noise 1.3 times that of its session' };
    const flagged: ImageCalibrationExplanation = {
      ...explanation,
      findings: [{ code: 'suspect_frames', level: 'check', reason: 'Its masters use 1 suspect frame', kinds: ['flat'] }],
      kinds: explanation.kinds.map((entry) => (entry.kind === 'flat' ? { ...entry, suspect: [suspectFlat] } : entry)),
    };
    show('/detail/8?db=c925&project=1', flagged, [{ image_id: 8, level: 'check', codes: ['suspect_frames'] }]);
    expect(await screen.findByText('Included frames graded Suspect')).toBeInTheDocument();
    expect(screen.getByTestId('calibration-panel')).toHaveTextContent('Check');
    cleanup();

    // Used anyway, the server counts the frame as OK: no finding, no
    // suspect list, no mark, so nothing here says Check.
    show('/detail/8?db=c925&project=1', { ...explanation, findings: [] }, []);
    await waitFor(() => expect(heading()).toHaveAccessibleName('Calibration no findings'));
    fireEvent.click(heading());
    expect(await screen.findByText(/calibrates this light with no findings/)).toBeInTheDocument();
    expect(screen.queryByText('Included frames graded Suspect')).toBeNull();
    expect(screen.getByTestId('calibration-panel')).not.toHaveTextContent('Check');
  });

  it('opens on a marked light and folds on a light with no finding, asking only when open', async () => {
    const view = show('/detail/8?db=c925&project=1', explanation, [thinDark]);
    expect(await screen.findByRole('button', { name: /only 3 frames/ })).toBeInTheDocument();
    expect(heading()).toHaveAttribute('aria-expanded', 'true');
    expect(asked).toEqual([8]);

    view.showImage(9);
    await waitFor(() => expect(heading()).toHaveAttribute('aria-expanded', 'false'));
    expect(heading()).toHaveAccessibleName('Calibration no findings');
    expect(screen.queryByRole('list', { name: 'Calibration findings' })).toBeNull();
    expect(asked).toEqual([8]);
  });

  it('shows a folded light its mark: the level and the findings by name', async () => {
    setDetailSection('calibration', 'closed');
    show('/detail/8?db=c925&project=1', explanation, [thinDark]);
    await waitFor(() =>
      expect(heading()).toHaveAccessibleName('Calibration Dark master has too few frames · Flats from several nights'));
    expect(screen.getByTestId('calibration-panel')).toHaveTextContent('Poor');
    expect(asked).toEqual([]);
  });

  it.each([
    ['a chip', '/detail/8?db=c925&project=1&calibFocus=thin_dark'],
    ['Images filtered by calibration', '/detail/8?db=c925&project=1&calib=poor'],
  ])('opens when the viewer came to look at calibration, from %s', async (_from, route) => {
    // Not marked, and folded by hand before: the purpose still opens it.
    setDetailSection('calibration', 'closed');
    show(route);
    expect(await screen.findByRole('button', { name: /only 3 frames/ })).toBeInTheDocument();
    expect(heading()).toHaveAttribute('aria-expanded', 'true');
    expect(asked).toEqual([8]);
  });

  it('keeps a hand-made choice from light to light over what the marks say', async () => {
    const view = show('/detail/8?db=c925&project=1', explanation, [thinDark, { ...thinDark, image_id: 10 }]);
    await screen.findByRole('button', { name: /only 3 frames/ });

    // Folded on a marked light, it stays folded on the next marked one.
    fireEvent.click(heading());
    expect(heading()).toHaveAttribute('aria-expanded', 'false');
    view.showImage(10);
    await waitFor(() => expect(heading()).toHaveAccessibleName(/^Calibration Dark master has too few frames/));
    expect(heading()).toHaveAttribute('aria-expanded', 'false');

    // Opened on a light with no finding, it stays open on the next one.
    view.showImage(9);
    fireEvent.click(heading());
    expect(heading()).toHaveAttribute('aria-expanded', 'true');
    view.showImage(11);
    await waitFor(() => expect(asked).toContain(11));
    expect(heading()).toHaveAttribute('aria-expanded', 'true');
    expect(asked).toEqual([8, 9, 11]);
  });

  it('folds a panel opened on purpose until the purpose changes', async () => {
    const view = show('/detail/8?db=c925&project=1&calibFocus=thin_dark');
    await screen.findByRole('button', { name: /only 3 frames/ });

    fireEvent.click(heading());
    expect(heading()).toHaveAttribute('aria-expanded', 'false');
    // The finding travels with the address from image to image; the panel
    // stays as the viewer left it.
    view.showImage(9);
    expect(heading()).toHaveAttribute('aria-expanded', 'false');
    expect(asked).toEqual([8]);

    // Another chip is another reason to look.
    act(() => go('/detail/9?db=c925&project=1&calibFocus=mixed_flats'));
    await waitFor(() => expect(heading()).toHaveAttribute('aria-expanded', 'true'));
    await waitFor(() => expect(asked).toEqual([8, 9]));
  });

  it('names the limits the light was checked by, what they kept from being flagged, and opens its rig', async () => {
    const opened: unknown[] = [];
    const listener = (event: Event) => opened.push((event as CustomEvent).detail);
    window.addEventListener('psf-guard:open-settings', listener);
    try {
      show('/detail/8?db=c925&project=1&calib=thin_dark', {
        ...explanation,
        findings: [explanation.findings[0]],
        waived: [{
          code: 'mixed_flats',
          level: 'check',
          reason: 'Its flat master mixes flats from 2 nights (check off, set for Askar 120 · Cam)',
          kinds: ['flat'],
        }],
        rules: {
          rig_uuid: '5a1e0000-0000-5000-8000-0000000000a1',
          rig_name: 'Askar 120 · Cam',
          rig_has_settings: true,
          notes: [
            { key: 'thin_master_frames', label: 'Minimum frames in bias and dark masters', value: 4, text: '4 frames', source: 'global' },
            { key: 'check_mixed_flats', label: 'Flats from several nights', value: 'off', text: 'off', source: 'rig' },
          ],
        },
      });
      const rules = await screen.findByTestId('calibration-rules');
      expect(within(rules).getByText('Not flagged')).toBeInTheDocument();
      expect(within(rules).getByText(/mixes flats from 2 nights \(check off, set for Askar 120/)).toBeInTheDocument();
      expect(within(rules).getByText("Checked by Askar 120 · Cam's own settings")).toBeInTheDocument();
      const notes = within(rules).getByRole('list', { name: 'Settings that differ from the defaults' });
      expect(within(notes).getAllByRole('listitem').map((item) => item.textContent)).toEqual([
        'Minimum frames in bias and dark masters4 framesglobal',
        'Flats from several nightsoffthis rig',
      ]);
      fireEvent.click(within(rules).getByRole('button', { name: 'Tune checks for Askar 120 · Cam' }));
      expect(opened).toEqual([
        { intent: 'calibration', rig: { uuid: '5a1e0000-0000-5000-8000-0000000000a1', name: 'Askar 120 · Cam' } },
      ]);
    } finally {
      window.removeEventListener('psf-guard:open-settings', listener);
    }
  });

  it('says every limit is at its default when nothing is set', async () => {
    show('/detail/8?db=c925&project=1&calib=thin_dark', {
      ...explanation,
      waived: [],
      rules: { rig_uuid: 'r', rig_name: 'Scope · Cam', rig_has_settings: false, notes: [] },
    });
    const rules = await screen.findByTestId('calibration-rules');
    expect(within(rules).getByText('Checked by the global settings')).toBeInTheDocument();
    expect(within(rules).getByText('Every limit is at its default.')).toBeInTheDocument();
    expect(within(rules).queryByText('Not flagged')).toBeNull();
  });
});
