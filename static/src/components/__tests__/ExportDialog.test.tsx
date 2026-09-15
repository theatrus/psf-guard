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
    fireEvent.change(screen.getByRole('textbox', { name: /PixInsight sees the image folders as/ }), {
      target: { value: '\\\\nas\\astro' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Start export' }));
    expect(onConfirm).toHaveBeenLastCalledWith({
      layout: 'wbpp',
      include_pending: true,
      placement: 'reference',
      remote_root: '\\\\nas\\astro',
    });

    // Back to the standard layout, the reference choice falls back to the
    // default placement rather than sending an export the server refuses.
    fireEvent.click(screen.getByRole('radio', { name: /Grouped by target/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Start export' }));
    expect(onConfirm.mock.lastCall?.[0]).toMatchObject({ placement: 'reflink' });
    expect(onConfirm.mock.lastCall?.[0]).not.toHaveProperty('remote_root');
  });

  it('offers a zip download only the layout and the ungraded choice', () => {
    render(
      <ExportDialog
        request={{ ...request, kind: 'download' }}
        defaultLayout="standard"
        busy={false}
        onClose={() => {}}
        onConfirm={() => {}}
      />
    );
    expect(screen.queryByRole('radio', { name: /Reference in place/ })).toBeNull();
    const download = () => screen.getByRole('link', { name: 'Download zip' }).getAttribute('href')!;
    expect(download()).toContain('include_pending=true');
    fireEvent.click(screen.getByRole('checkbox', { name: /Include ungraded lights/ }));
    expect(download()).not.toContain('include_pending');
  });
});
