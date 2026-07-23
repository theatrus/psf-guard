import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import ProjectSchedulerDialog from '../ProjectSchedulerDialog';

const project = {
  id: 7,
  profile_id: 'profile-1',
  name: 'M31 season',
  description: 'Andromeda data',
  state: 1,
  priority: 2,
  created_at: 1_700_000_000,
  active_at: null,
  inactive_at: null,
  minimum_time: 30,
  minimum_altitude: 25,
  maximum_altitude: 0,
  use_custom_horizon: false,
  horizon_offset: 0,
  meridian_window: 0,
  filter_switch_frequency: 0,
  dither_every: 3,
  enable_grader: true,
  is_mosaic: false,
  exposure_templates: [{
    id: 31,
    profile_id: 'profile-1',
    name: 'Ha template',
    filter_name: 'Ha',
    gain: 100,
    offset: 30,
    bin: 1,
    readout_mode: null,
    twilight_level: 0,
    moon_avoidance_enabled: true,
    moon_avoidance_separation: 60,
    moon_avoidance_width: 7,
    maximum_humidity: 75,
    default_exposure: 300,
    moon_relax_scale: 0,
    moon_relax_max_altitude: 5,
    moon_relax_min_altitude: -15,
    moon_down_enabled: false,
    dither_every: -1,
    minutes_offset: 0,
    plan_count: 1,
  }],
  targets: [{
    id: 11,
    name: 'M31',
    active: true,
    ra_hours: 0.712313,
    dec_degrees: 41.2687,
    epoch_code: 2,
    rotation: 0,
    roi: 100,
    exposure_plans: [{
      id: 21,
      exposure_template_id: 31,
      template_name: 'Ha template',
      filter_name: 'Ha',
      gain: 100,
      offset: 30,
      bin: 1,
      readout_mode: null,
      exposure: 300,
      desired: 40,
      acquired: 12,
      accepted: 10,
      enabled: true,
    }],
  }],
};

function renderDialog(canEdit = true) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <ProjectSchedulerDialog
        open
        dbId="db-test"
        projectId={7}
        projectName="M31 season"
        canEdit={canEdit}
        onClose={() => undefined}
      />
    </QueryClientProvider>
  );
}

describe('ProjectSchedulerDialog', () => {
  it('shows Target Scheduler coordinates and exposure counts', async () => {
    server.use(
      http.get('/api/db/db-test/projects/7/scheduler', () =>
        HttpResponse.json({ success: true, data: project, error: null, status: 'ready' })
      )
    );
    renderDialog(false);

    expect(await screen.findByText('M31')).toBeInTheDocument();
    expect(screen.getByText(/00h 42m 44\.3s/)).toBeInTheDocument();
    expect(screen.getAllByText('Ha template')).toHaveLength(2);
    expect(screen.getByText('300s')).toBeInTheDocument();
    expect(screen.getByText(/Twilight 0 · humidity 75/)).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: '12' })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: '10' })).toBeInTheDocument();
    expect(screen.getByText(/View only/)).toBeInTheDocument();
  });

  it('creates a plan with template inputs kept separate from counts', async () => {
    let posted: unknown = null;
    server.use(
      http.get('/api/db/db-test/projects/7/scheduler', () =>
        HttpResponse.json({ success: true, data: project, error: null, status: 'ready' })
      ),
      http.post('/api/db/db-test/targets/11/exposure-plans', async ({ request }) => {
        posted = await request.json();
        return HttpResponse.json({
          success: true,
          data: { ...project.targets[0].exposure_plans[0], id: 22, filter_name: 'OIII' },
          error: null,
          status: 'ready',
        });
      })
    );
    const user = userEvent.setup();
    renderDialog();

    await screen.findByText('M31');
    await user.click(screen.getByRole('button', { name: 'Add plan' }));
    await user.type(screen.getByLabelText('Filter'), 'OIII');
    await user.click(screen.getByRole('button', { name: 'Create plan' }));

    await waitFor(() => expect(posted).not.toBeNull());
    expect(posted).toMatchObject({
      filter_name: 'OIII',
      exposure: 60,
      desired: 1,
      bin: 1,
      enabled: true,
    });
  });

  it('reuses an exposed profile template by id', async () => {
    let posted: unknown = null;
    server.use(
      http.get('/api/db/db-test/projects/7/scheduler', () =>
        HttpResponse.json({ success: true, data: project, error: null, status: 'ready' })
      ),
      http.post('/api/db/db-test/targets/11/exposure-plans', async ({ request }) => {
        posted = await request.json();
        return HttpResponse.json({
          success: true,
          data: { ...project.targets[0].exposure_plans[0], id: 22 },
          error: null,
          status: 'ready',
        });
      })
    );
    const user = userEvent.setup();
    renderDialog();

    await screen.findByText('M31');
    await user.click(screen.getByRole('button', { name: 'Add plan' }));
    await user.selectOptions(screen.getByLabelText('Exposure template'), '31');
    expect(screen.getByLabelText('Filter')).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Create plan' }));

    await waitFor(() => expect(posted).not.toBeNull());
    expect(posted).toMatchObject({
      exposure_template_id: 31,
      filter_name: 'Ha',
      gain: 100,
      offset: 30,
      bin: 1,
    });
  });
});
