import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import PathField from '../PathField';

const desktop = vi.hoisted(() => ({ isTauri: false, picked: '/picked/folder' as string | null }));
vi.mock('../../utils/tauri', () => ({
  isTauriApp: () => desktop.isTauri,
  tauriFileSystem: {
    pickPath: vi.fn(async () => desktop.picked),
  },
}));

afterEach(() => {
  desktop.isTauri = false;
  desktop.picked = '/picked/folder';
});

describe('PathField', () => {
  it('is a plain text field in the browser', () => {
    const onChange = vi.fn();
    render(<PathField aria-label="Folder" value="/a" onChange={onChange} />);
    fireEvent.change(screen.getByLabelText('Folder'), { target: { value: '/b' } });
    expect(onChange).toHaveBeenCalledWith('/b');
    expect(screen.queryByRole('button', { name: 'Browse…' })).not.toBeInTheDocument();
  });

  it('fills in what the desktop dialog picked, or keeps the value when cancelled', async () => {
    desktop.isTauri = true;
    const onChange = vi.fn();
    render(<PathField aria-label="Folder" value="/a" onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: 'Browse…' }));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith('/picked/folder'));

    onChange.mockClear();
    desktop.picked = null;
    fireEvent.click(screen.getByRole('button', { name: 'Browse…' }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(onChange).not.toHaveBeenCalled();
  });

  it('uses a picker of its own when given one', async () => {
    desktop.isTauri = true;
    const pick = vi.fn(async () => '/db.sqlite');
    const onChange = vi.fn();
    render(<PathField aria-label="Database" kind="file" pick={pick} value="" onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: 'Browse…' }));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith('/db.sqlite'));
    expect(pick).toHaveBeenCalled();
  });
});
