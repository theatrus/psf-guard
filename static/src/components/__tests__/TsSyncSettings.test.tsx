import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { HttpResponse, http } from 'msw';
import { expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import TsSyncSettings from '../TsSyncSettings';

const settings = { source_path: '/scope/TS.sqlite', writeback: true, on_open: true, on_change: true, interval_minutes: 5 };

it('saves independent automatic sync controls and retains edits across an unchanged refresh', async () => {
  const saved = vi.fn();
  const onSaved = vi.fn(async () => undefined);
  server.use(http.put('/api/databases/rig', async ({ request }) => {
    saved(await request.json());
    return HttpResponse.json({ success: true, data: { id: 'rig' } });
  }));
  const { rerender } = render(<TsSyncSettings dbId="rig" settings={settings} onSaved={onSaved} />);
  fireEvent.click(screen.getByLabelText('Automatically write grade changes back to Target Scheduler'));
  fireEvent.change(screen.getByLabelText('Sync interval (minutes; 0 disables the schedule)'), { target: { value: '0' } });
  rerender(<TsSyncSettings dbId="rig" settings={{ ...settings }} onSaved={onSaved} />);
  expect(screen.getByLabelText('Automatically write grade changes back to Target Scheduler')).not.toBeChecked();
  expect(screen.getByLabelText('Sync when new images or grades arrive')).toBeChecked();
  fireEvent.click(screen.getByRole('button', { name: 'Save sync settings' }));
  await waitFor(() => expect(saved).toHaveBeenCalledWith({ ts_sync: { ...settings, writeback: false, interval_minutes: 0 } }));
  expect(onSaved).toHaveBeenCalledOnce();
});
