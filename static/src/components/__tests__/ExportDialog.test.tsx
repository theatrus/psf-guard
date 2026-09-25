import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import ExportDialog from '../ExportDialog';

const request = {
  dbId: 'alpha',
  scope: { project_id: 1 },
  label: 'Sh2 86',
};

describe('ExportDialog', () => {
  it('includes ungraded lights by default and lets the export leave them out', () => {
    const onConfirm = vi.fn();
    render(
      <ExportDialog
        request={{ ...request, kind: 'server' }}
        defaultLayout="wbpp"
        busy={false}
        onClose={() => {}}
        onConfirm={onConfirm}
      />
    );
    const pending = screen.getByRole('checkbox', { name: /Include ungraded lights/ });
    expect(pending).toBeChecked();
    fireEvent.click(screen.getByRole('button', { name: 'Start export' }));
    expect(onConfirm).toHaveBeenLastCalledWith({
      layout: 'wbpp',
      include_pending: true,
      placement: 'reflink',
      wbpp: { quality: 'maximum', fast_integration: 'off', drizzle: 'off' },
    });

    fireEvent.click(pending);
    fireEvent.click(screen.getByRole('button', { name: 'Start export' }));
    expect(onConfirm.mock.lastCall?.[0]).toMatchObject({ include_pending: false });
  });

  it('references frames in place only with the WBPP layout, with an optional remote root', () => {
    const onConfirm = vi.fn();
    render(
      <ExportDialog
        request={{ ...request, kind: 'server' }}
        defaultLayout="standard"
        sourceRoot="/mnt/nas/astro"
        busy={false}
        onClose={() => {}}
        onConfirm={onConfirm}
      />
    );
    const reference = screen.getByRole('radio', { name: /Reference in place/ });
    expect(reference).toBeDisabled();

    fireEvent.click(screen.getByRole('radio', { name: /^WBPP/ }));
    expect(reference).toBeEnabled();
    fireEvent.click(reference);
    // The server side is prefilled from the database's image folders.
    expect(screen.getByRole('textbox', { name: /^Server path/ })).toHaveValue(
      '/mnt/nas/astro'
    );
    fireEvent.change(screen.getByRole('textbox', { name: /^PixInsight path/ }), {
      target: { value: 'P:\\' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start export' }));
    expect(onConfirm).toHaveBeenLastCalledWith({
      layout: 'wbpp',
      include_pending: true,
      placement: 'reference',
      local_root: '/mnt/nas/astro',
      remote_root: 'P:\\',
      wbpp: { quality: 'maximum', fast_integration: 'off', drizzle: 'off' },
    });

    // Back to the standard layout, the reference choice falls back to the
    // default placement rather than sending an export the server refuses.
    fireEvent.click(screen.getByRole('radio', { name: /Grouped by target/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Start export' }));
    expect(onConfirm.mock.lastCall?.[0]).toMatchObject({ placement: 'reflink' });
    expect(onConfirm.mock.lastCall?.[0]).not.toHaveProperty('remote_root');
    expect(onConfirm.mock.lastCall?.[0]).not.toHaveProperty('local_root');
    // The standard layout has no runner, so it carries no WBPP settings.
    expect(onConfirm.mock.lastCall?.[0]).not.toHaveProperty('wbpp');
  });

  it('carries the chosen WBPP settings with the WBPP layout, and in a download link', () => {
    const onConfirm = vi.fn();
    render(
      <ExportDialog
        request={{ ...request, kind: 'server' }}
        defaultLayout="wbpp"
        defaultWbpp={{ quality: 'good', fast_integration: 'auto', drizzle: 'off' }}
        busy={false}
        onClose={() => {}}
        onConfirm={onConfirm}
      />
    );
    fireEvent.change(screen.getByLabelText(/^Drizzle/), { target: { value: '2x' } });
    fireEvent.change(screen.getByLabelText(/^Autocrop/), { target: { value: 'off' } });
    fireEvent.click(screen.getByRole('button', { name: 'Start export' }));
    expect(onConfirm.mock.lastCall?.[0]).toMatchObject({
      wbpp: { quality: 'good', fast_integration: 'auto', drizzle: '2x', autocrop: false },
    });
  });

  it('lets a zip download carry the scripts alone, naming the frames in place', () => {
    render(
      <ExportDialog
        request={{ ...request, kind: 'download' }}
        defaultLayout="wbpp"
        sourceRoot="/mnt/nas/astro"
        busy={false}
        onClose={() => {}}
        onConfirm={() => {}}
      />
    );
    const download = () => screen.getByRole('link', { name: 'Download zip' }).getAttribute('href')!;
    expect(download()).toContain('include_pending=true');
    expect(download()).toContain('wbpp_quality=maximum');
    expect(download()).toContain('wbpp_fast_integration=off');
    expect(download()).not.toContain('placement=');
    fireEvent.click(screen.getByRole('checkbox', { name: /Include ungraded lights/ }));
    expect(download()).not.toContain('include_pending');

    fireEvent.click(screen.getByRole('radio', { name: /Scripts only/ }));
    fireEvent.change(screen.getByRole('textbox', { name: /^PixInsight path/ }), {
      target: { value: 'P:\\' },
    });
    const href = download();
    expect(href).toContain('placement=reference');
    expect(href).toContain('local_root=%2Fmnt%2Fnas%2Fastro');
    expect(href).toContain('remote_root=P%3A%5C');

    // The standard layout has no runner, so the scripts-only choice is not on offer.
    fireEvent.click(screen.getByRole('radio', { name: /Grouped by target/ }));
    expect(screen.getByRole('radio', { name: /Scripts only/ })).toBeDisabled();
    expect(download()).not.toContain('placement=');
    expect(download()).not.toContain('wbpp_');
  });
});

describe('commonDirectory', () => {
  it('finds the folder a database\u2019s image directories share', async () => {
    const { commonDirectory } = await import('../../utils/commonDirectory');
    expect(
      commonDirectory([
        '/mnt/barium/astrobin/_ByTelescope/starfront-ultracat131/_Source',
        '/mnt/barium/astrobin/_Incoming/starfront-ultracat131',
        '/mnt/barium/astrobin/_Calibration/starfront-ultracat131',
      ])
    ).toBe('/mnt/barium/astrobin');
    expect(commonDirectory(['/data/a'])).toBe('/data/a');
    expect(commonDirectory(['/data/a', '/srv/b'])).toBe('');
    expect(commonDirectory(['D:\\pictures\\a', 'D:\\pictures\\b'])).toBe('D:\\pictures');
    expect(commonDirectory([])).toBe('');
  });
});
